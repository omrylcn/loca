//! The key ledger: every code issued is recorded, never its secret.
use super::*;

const MASTER: &str = "keys-master-key";

async fn post_json(base: &str, path: &str, admin: &str, body: Value) -> (u16, Value) {
    let r = reqwest::Client::new()
        .post(format!("{base}{path}"))
        .header("x-admin-token", admin)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

async fn ledger(base: &str, admin: &str) -> (u16, String, Vec<Value>) {
    let r = reqwest::Client::new()
        .get(format!("{base}/keys"))
        .header("x-admin-token", admin)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    let rows = serde_json::from_str::<Vec<Value>>(&text).unwrap_or_default();
    (status, text, rows)
}

#[tokio::test]
async fn every_issued_code_is_in_the_ledger_and_no_secret_ever_is() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("keys.db");
    let (port, _guard) =
        spawn_server_env(MASTER, &[("DB_PATH", db.to_str().unwrap().into())]).await;
    let base = format!("http://127.0.0.1:{port}");

    // A Master code from the one generator: recorded as unused, fingerprint only.
    let (status, first) = post_json(
        &base,
        "/keys",
        MASTER,
        serde_json::json!({ "kind": "master" }),
    )
    .await;
    assert_eq!(status, 201, "{first}");
    let first_code = first["code"].as_str().unwrap().to_string();
    let (_, text, rows) = ledger(&base, MASTER).await;
    assert!(
        !text.contains(&first_code),
        "the ledger never holds a secret"
    );
    let master_rows: Vec<&Value> = rows.iter().filter(|k| k["kind"] == "master").collect();
    assert_eq!(master_rows.len(), 1);
    assert_eq!(master_rows[0]["state"], "unused");
    assert!(first_code.starts_with(
        master_rows[0]["fingerprint"]
            .as_str()
            .unwrap()
            .trim_end_matches('…')
    ));

    // A second Master code replaces the unused first, and the ledger says so.
    let (_, second) = post_json(
        &base,
        "/keys",
        MASTER,
        serde_json::json!({ "kind": "master" }),
    )
    .await;
    let (_, _, rows) = ledger(&base, MASTER).await;
    let states: Vec<&str> = rows
        .iter()
        .filter(|k| k["kind"] == "master")
        .map(|k| k["state"].as_str().unwrap())
        .collect();
    assert!(
        states.contains(&"superseded") && states.contains(&"unused"),
        "{states:?}"
    );
    // Using it marks it used.
    let signed = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header("x-pairing-code", second["code"].as_str().unwrap())
        .json(&serde_json::json!({ "name": "", "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert!(signed.status().is_success());
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert!(rows
        .iter()
        .any(|k| k["kind"] == "master" && k["state"] == "used"));
    // The old path (/pairings) is recorded too — there is no unrecorded door.
    reqwest::Client::new()
        .post(format!("{base}/pairings?ttl_hours=1"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert_eq!(rows.iter().filter(|k| k["kind"] == "master").count(), 3);

    // A Smaster key may come without a name (the person types it at sign-in);
    // a title is never a name.
    let (status, unnamed) = post_json(
        &base,
        "/keys",
        MASTER,
        serde_json::json!({ "kind": "smaster" }),
    )
    .await;
    assert_eq!(status, 201, "{unnamed}");
    assert!(unnamed["name"].as_str().unwrap().starts_with("yeni-"));
    assert_eq!(
        post_json(
            &base,
            "/keys",
            MASTER,
            serde_json::json!({ "kind": "smaster", "name": "smaster" })
        )
        .await
        .0,
        400
    );
    let (status, ayse) = post_json(
        &base,
        "/keys",
        MASTER,
        serde_json::json!({ "kind": "smaster", "name": "ayse" }),
    )
    .await;
    assert_eq!(status, 201, "{ayse}");
    assert_eq!(
        post_json(
            &base,
            "/keys",
            MASTER,
            serde_json::json!({ "kind": "smaster", "name": "AYSE" })
        )
        .await
        .0,
        409,
        "one name, one person"
    );
    let ayse_code = ayse["code"].as_str().unwrap().to_string();
    let (_, text, rows) = ledger(&base, MASTER).await;
    assert!(!text.contains(&ayse_code));
    let row = rows
        .iter()
        .find(|k| k["kind"] == "smaster" && k["for_name"] == "ayse")
        .unwrap();
    assert_eq!(row["state"], "unused");
    assert!(row["use_by"].as_u64().unwrap() > row["issued_at"].as_u64().unwrap());
    // The Building says it is a code, not a person, until someone signs in.
    let ranks: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/building/ranks"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        ranks.iter().find(|r| r["name"] == "ayse").unwrap()["unused"],
        true
    );
    let used = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header("x-admin-token", &ayse_code)
        .json(&serde_json::json!({ "name": "ayse", "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert!(used.status().is_success());
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert_eq!(
        rows.iter().find(|k| k["for_name"] == "ayse").unwrap()["state"],
        "active"
    );

    // Only the Master reads the ledger or issues from it.
    assert_eq!(ledger(&base, &ayse_code).await.0, 403);
    assert_eq!(
        post_json(
            &base,
            "/keys",
            &ayse_code,
            serde_json::json!({ "kind": "master" })
        )
        .await
        .0,
        403
    );

    // The Master names a Smaster; revoking from the ledger works.
    let id = row["id"].as_str().unwrap().to_string();
    let renamed = reqwest::Client::new()
        .put(format!("{base}/smasters/{id}"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "ayse-k" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status().as_u16(), 200);
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert!(rows.iter().any(|k| k["for_name"] == "ayse-k"));
    let revoked = reqwest::Client::new()
        .delete(format!("{base}/keys/{id}"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status().as_u16(), 200);
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert_eq!(
        rows.iter().find(|k| k["id"] == id.as_str()).unwrap()["state"],
        "revoked"
    );
}

async fn status_with(method: reqwest::Method, url: String, header: (&str, &str)) -> u16 {
    reqwest::Client::new()
        .request(method, url)
        .header(header.0, header.1)
        .json(&serde_json::json!({ "name": "probe", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn admin_ws_can(port: u16, room: &str, credential: &str) -> bool {
    let mut request = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=user")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.admin.{credential}")).unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.is_ok()
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// What the registry keeps about a Smaster code (its hash), and the ids that
/// name the Smaster (`pr_`, `smid_`), are not keys: none of them signs in
/// anywhere the code does.
#[tokio::test]
async fn a_smaster_code_hash_principal_or_management_id_is_not_a_key() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("not-a-key.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, MASTER, "probe").await;
    let (status, minted) = post_json(
        &base,
        "/smasters",
        MASTER,
        serde_json::json!({ "name": "notakey" }),
    )
    .await;
    assert_eq!(status, 201);
    let code = minted["token"].as_str().unwrap().to_string();
    let profile: Value = reqwest::Client::new()
        .get(format!("{base}/profile"))
        .header("x-admin-token", &code)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let principal = profile["principal"]["id"].as_str().unwrap().to_string();
    let listed: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/smasters"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let management = listed.iter().find(|s| s["name"] == "notakey").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(principal.starts_with("pr_") && management.starts_with("smid_"));

    // The code itself is the positive control.
    let probes = [
        ("code", code.clone(), true),
        ("hash", sha256_hex(&code), false),
        ("principal", principal, false),
        ("management id", management, false),
    ];
    for (what, key, works) in probes {
        let ok = |status: u16| (200..300).contains(&status);
        let mut answers = Vec::new();
        for header in ["x-admin-token", "x-room-token"] {
            answers.push((
                format!("{header} at /profile"),
                ok(status_with(
                    reqwest::Method::GET,
                    format!("{base}/profile"),
                    (header, &key),
                )
                .await),
            ));
            answers.push((
                format!("{header} at /sessions"),
                ok(status_with(
                    reqwest::Method::POST,
                    format!("{base}/sessions"),
                    (header, &key),
                )
                .await),
            ));
            answers.push((
                format!("{header} reading a loca"),
                ok(rest_can(&base, "probe", &[(header, &key)]).await),
            ));
        }
        answers.push((
            "Building authority".into(),
            ok(status_with(
                reqwest::Method::GET,
                format!("{base}/building/ranks"),
                ("x-admin-token", &key),
            )
            .await),
        ));
        answers.push((
            "WebSocket admin subprotocol".into(),
            admin_ws_can(port, "probe", &key).await,
        ));
        answers.push((
            "legacy WebSocket query".into(),
            ws_can(port, "probe", &format!("admin={key}")).await,
        ));
        if works {
            for (door, answer) in &answers {
                if door.starts_with("x-admin-token") || !door.starts_with("x-") {
                    assert!(answer, "the code itself opens {door}");
                }
            }
        } else {
            let opened: Vec<_> = answers
                .iter()
                .filter(|(_, answer)| *answer)
                .map(|(door, _)| door.as_str())
                .collect();
            assert!(opened.is_empty(), "the {what} opened {opened:?}");
        }
    }
}

/// Take a snapshot of a database the upgrade migrated, restore it elsewhere
/// and open it: the same code signs in as the same principal, and neither
/// the database nor the snapshot holds the code.
#[tokio::test]
async fn a_migrated_registry_survives_snapshot_restore_and_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("loca.db");
    let code = "sm_QZXJ-synthetic-snapshot-code-7777777777777777777777777777777777";
    let tail = &code[7..];
    // Issued now: the seven-day sweep would take back an unused older code.
    let issued_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    // A registry from before identity v2, written as v0.13.0 did.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TABLE smasters (token TEXT PRIMARY KEY, name TEXT NOT NULL,
                                    issued_at INTEGER NOT NULL, revoked_at INTEGER);
             INSERT INTO smasters VALUES ('{code}', 'snapper', {issued_at}, NULL);"
        ))
        .unwrap();
    let env = |path: &std::path::Path| vec![("DB_PATH", path.to_string_lossy().into_owned())];
    let principal_of = |base: String| async move {
        let response = reqwest::Client::new()
            .get(format!("{base}/profile"))
            .header("x-admin-token", code)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), 200, "the code signs in");
        let profile: Value = response.json().await.unwrap();
        profile["principal"]["id"].as_str().unwrap().to_string()
    };
    let expected = {
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(format!("smaster:{code}").as_bytes());
        format!(
            "pr_{}",
            digest[..16]
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        )
    };

    let (port, guard) = spawn_server_env(MASTER, &env(&db)).await;
    assert_eq!(
        principal_of(format!("http://127.0.0.1:{port}")).await,
        expected
    );
    drop(guard);
    wait_for_server_shutdown(port).await;

    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/storage_snapshot.py");
    let snapshot = dir.path().join("snapshot");
    let restored = dir.path().join("restored");
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    for (action, source, target) in [
        ("backup", db.as_path(), snapshot.as_path()),
        ("restore", snapshot.as_path(), restored.as_path()),
    ] {
        let result = std::process::Command::new(&python)
            .arg(&script)
            .arg(action)
            .arg(source)
            .arg(target)
            .arg("--server-stopped")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{action}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    for file in [db.clone(), snapshot.join("loca.sqlite3")] {
        let bytes = std::fs::read(&file).unwrap();
        assert!(
            !bytes.windows(tail.len()).any(|w| w == tail.as_bytes()),
            "{} holds the code",
            file.display()
        );
    }
    let restored_db = restored.join("loca.sqlite3");
    let (port, _guard) = spawn_server_env(MASTER, &env(&restored_db)).await;
    assert_eq!(
        principal_of(format!("http://127.0.0.1:{port}")).await,
        expected,
        "the restored copy signs the same code in as the same principal"
    );
}

/// The Master manages a Smaster by its `smid_` (or the holder presents the
/// code itself, which is hashed and compared). A hash or a `pr_` names
/// nothing, so it can neither rename nor revoke.
#[tokio::test]
async fn smaster_management_takes_a_smid_or_the_code_never_a_hash_or_principal() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("manage.db");
    let (port, _guard) =
        spawn_server_env(MASTER, &[("DB_PATH", db.to_str().unwrap().into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mut minted = Vec::new();
    for name in ["first", "second"] {
        let (status, body) = post_json(
            &base,
            "/keys",
            MASTER,
            serde_json::json!({ "kind": "smaster", "name": name }),
        )
        .await;
        assert_eq!(status, 201);
        minted.push(body["code"].as_str().unwrap().to_string());
    }
    let profile: Value = client
        .get(format!("{base}/profile"))
        .header("x-admin-token", &minted[0])
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let principal = profile["principal"]["id"].as_str().unwrap().to_string();
    let (_, _, rows) = ledger(&base, MASTER).await;
    let smid = rows.iter().find(|k| k["for_name"] == "first").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    for reference in [sha256_hex(&minted[0]), principal] {
        let renamed = client
            .put(format!("{base}/smasters/{reference}"))
            .header("x-admin-token", MASTER)
            .json(&serde_json::json!({ "name": "stolen" }))
            .send()
            .await
            .unwrap();
        assert_eq!(renamed.status().as_u16(), 404);
        for path in ["smasters", "keys"] {
            let revoked = client
                .delete(format!("{base}/{path}/{reference}"))
                .header("x-admin-token", MASTER)
                .send()
                .await
                .unwrap();
            assert_eq!(revoked.status().as_u16(), 404, "/{path}/<not a reference>");
        }
    }
    let (_, _, rows) = ledger(&base, MASTER).await;
    assert!(rows
        .iter()
        .filter(|k| k["kind"] == "smaster")
        .all(|k| k["state"] != "revoked" && k["for_name"] != "stolen"));
    // By its smid_: renamed, then revoked.
    let renamed = client
        .put(format!("{base}/smasters/{smid}"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": "first-k" }))
        .send()
        .await
        .unwrap();
    assert_eq!(renamed.status().as_u16(), 200);
    let revoked = client
        .delete(format!("{base}/keys/{smid}"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status().as_u16(), 200);
    // By the code itself.
    let revoked = client
        .delete(format!("{base}/smasters/{}", minted[1]))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status().as_u16(), 200);
    let (_, text, rows) = ledger(&base, MASTER).await;
    for code in &minted {
        assert!(!text.contains(code.as_str()));
    }
    let states: Vec<_> = rows
        .iter()
        .filter(|k| k["kind"] == "smaster")
        .map(|k| (k["for_name"].clone(), k["state"].clone()))
        .collect();
    assert_eq!(states.len(), 2);
    assert!(
        states.iter().all(|(_, state)| state == "revoked"),
        "{states:?}"
    );
    assert!(states.iter().any(|(name, _)| name == "first-k"));
}

/// "No keys were ever issued" and "the record could not be read" must not look
/// the same to the Master: an unreadable ledger is an error, not an empty one.
#[tokio::test]
async fn an_unreadable_key_ledger_is_an_error_not_an_empty_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("keys-unreadable.db");
    let (port, _guard) =
        spawn_server_env(MASTER, &[("DB_PATH", db.to_str().unwrap().into())]).await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, _) = post_json(
        &base,
        "/keys",
        MASTER,
        serde_json::json!({ "kind": "smaster", "name": "ada" }),
    )
    .await;
    assert_eq!(status, 201);
    let (status, _, rows) = ledger(&base, MASTER).await;
    assert_eq!(status, 200);
    assert!(!rows.is_empty(), "the issued key is recorded");

    // The ledger's own table goes missing under the running server.
    rusqlite::Connection::open(&db)
        .unwrap()
        .execute_batch("DROP TABLE pairing_log")
        .unwrap();
    let (status, text, rows) = ledger(&base, MASTER).await;
    assert_eq!(status, 503, "{text}");
    assert!(rows.is_empty());
    assert!(text.contains("could not be read"), "{text}");
}
