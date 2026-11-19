//! Who minted an admission right, as the key ledger and the stock record it.
//!
//! PRINCIPLES "Groups" ("Every change is recorded with who made it") and the
//! Smaster definition ("Building layer"): a Smaster may mint admission stock
//! (`POST /admission-stock`), and the right it mints is the Smaster's, not the
//! Master's.
use super::*;

const MASTER: &str = "stock-master-key";

async fn send(
    method: reqwest::Method,
    url: String,
    header: (&str, &str),
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new()
        .request(method, url)
        .header(header.0, header.1);
    if let Some(body) = body {
        req = req.json(&body);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn admission_rows(ledger: &Value) -> Vec<String> {
    ledger
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["kind"] == "admission")
        .map(|row| row["issued_by"].as_str().unwrap_or("").to_string())
        .collect()
}

#[tokio::test]
async fn admission_stock_names_the_rank_that_minted_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("stock-minter.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    let (status, sm) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    assert_eq!(status, 201, "{sm}");
    let key = sm["token"].as_str().unwrap().to_string();
    let (status, s) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        ("x-admin-token", &key),
        Some(serde_json::json!({ "name": "deputy", "kind": "user" })),
    )
    .await;
    assert_eq!(status, 201, "{s}");
    let session = s["session_token"].as_str().unwrap().to_string();

    // The Smaster mints two rights through its session.
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/admission-stock"),
        ("x-session-token", &session),
        Some(serde_json::json!({ "count": 2 })),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let (_, ledger) = send(
        reqwest::Method::GET,
        format!("{base}/keys"),
        ("x-admin-token", MASTER),
        None,
    )
    .await;
    assert_eq!(
        admission_rows(&ledger),
        vec!["smaster:deputy", "smaster:deputy"],
        "the ledger names the Smaster that minted the rights"
    );
    let smaster_principal: String = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT principal_id FROM smasters WHERE name = 'deputy'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let minted_by: Vec<String> = {
        let c = rusqlite::Connection::open(&db).unwrap();
        let mut st = c.prepare("SELECT minted_by FROM admission_stock").unwrap();
        st.query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    };
    assert_eq!(
        minted_by,
        vec![smaster_principal.clone(), smaster_principal]
    );

    // The Master's own right is still the Master's, through either door.
    let (status, _) = send(
        reqwest::Method::POST,
        format!("{base}/admission-stock"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "count": 1 })),
    )
    .await;
    assert_eq!(status, 201);
    let (status, _) = send(
        reqwest::Method::POST,
        format!("{base}/keys"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "kind": "admission", "count": 1 })),
    )
    .await;
    assert_eq!(status, 201);
    let (_, ledger) = send(
        reqwest::Method::GET,
        format!("{base}/keys"),
        ("x-admin-token", MASTER),
        None,
    )
    .await;
    let mut rows = admission_rows(&ledger);
    rows.sort();
    assert_eq!(
        rows,
        vec!["Master", "Master", "smaster:deputy", "smaster:deputy"]
    );
}
