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
