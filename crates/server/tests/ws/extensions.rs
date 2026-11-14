//! Loca extensions (PRINCIPLES "Loca extensions"): a loca's chosen set, who
//! changes it, the server refusing what is off, and the Journal recording it.
use super::*;

const MASTER: &str = "ext-master-key";

async fn call(
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

async fn member(base: &str, name: &str, kind: &str) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/members"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "admit {name}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        header,
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "session {name}: {status} {v}");
    v["session_token"].as_str().unwrap().to_string()
}

async fn seat(base: &str, room: &str, name: &str) {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/invites"),
        ("x-admin-token", MASTER),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert!(status < 300, "seat {name} in {room}: {status} {v}");
}

async fn create(base: &str, room: &str, extensions: Option<&[&str]>) -> (u16, Value) {
    let mut body = serde_json::json!({ "name": room });
    if let Some(list) = extensions {
        body["extensions"] = serde_json::json!(list);
    }
    call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ("x-admin-token", MASTER),
        Some(body),
    )
    .await
}

fn enabled(v: &Value) -> Vec<String> {
    v["enabled"]
        .as_array()
        .unwrap_or_else(|| panic!("no enabled list in {v}"))
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

async fn extensions_of(base: &str, room: &str, header: (&str, &str)) -> Value {
    let (status, v) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/extensions"),
        header,
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v
}

async fn journal_texts(base: &str, room: &str) -> Vec<String> {
    // Read the store directly through a loca whose journal is on, or via the
    // route when it is; the caller turns journal on before asking.
    let (status, v) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/journal"),
        ("x-admin-token", MASTER),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} | {}",
                e["by"].as_str().unwrap_or(""),
                e["text"].as_str().unwrap_or("")
            )
        })
        .collect()
}

/// Refused by the extension fence (as opposed to a handler's own 403).
fn fenced(status: u16, v: &Value) -> bool {
    status == 403
        && v.as_str()
            .is_some_and(|text| text.contains("this extension is off in this loca"))
}

/// Every extension route of each kind, as (method, path, body).
fn probes(room: &str) -> Vec<(&'static str, reqwest::Method, String, Option<Value>)> {
    use reqwest::Method;
    vec![
        ("focus", Method::GET, format!("/rooms/{room}/tasks"), None),
        (
            "focus",
            Method::POST,
            format!("/rooms/{room}/tasks"),
            Some(serde_json::json!({"title":"t"})),
        ),
        ("focus", Method::GET, format!("/rooms/{room}/goals"), None),
        (
            "focus",
            Method::POST,
            format!("/rooms/{room}/goals"),
            Some(serde_json::json!({"outcome":"o"})),
        ),
        ("notes", Method::GET, format!("/rooms/{room}/notes"), None),
        (
            "notes",
            Method::POST,
            format!("/rooms/{room}/notes"),
            Some(serde_json::json!({"key":"k","title":"t","body":"b","by":"x"})),
        ),
        ("notes", Method::GET, format!("/rooms/{room}/notes/k"), None),
        (
            "journal",
            Method::GET,
            format!("/rooms/{room}/journal"),
            None,
        ),
        (
            "journal",
            Method::POST,
            format!("/rooms/{room}/journal"),
            Some(serde_json::json!({"text":"did it"})),
        ),
        ("wiki", Method::GET, format!("/rooms/{room}/wiki"), None),
        (
            "wiki",
            Method::POST,
            format!("/rooms/{room}/wiki/review"),
            Some(serde_json::json!({})),
        ),
        (
            "wiki",
            Method::PUT,
            format!("/rooms/{room}/wiki/config"),
            Some(serde_json::json!({})),
        ),
    ]
}

#[tokio::test]
async fn extensions_are_chosen_enforced_governed_and_journaled() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("extensions.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let admin = ("x-admin-token", MASTER);
    use reqwest::Method;

    // The catalog: five of today's modules; notes, journal and wiki are
    // ticked for a new loca; chat is not an extension.
    let (status, catalog) = call(Method::GET, format!("{base}/extensions"), admin, None).await;
    assert_eq!(status, 200);
    let ids: Vec<&str> = catalog["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["notes", "focus", "journal", "wiki", "live"]);
    assert_eq!(
        catalog["defaults"],
        serde_json::json!(["notes", "journal", "wiki"])
    );

    // A new loca gets exactly the chosen set; unknown names are refused.
    assert_eq!(create(&base, "bad", Some(&["notes", "chat"])).await.0, 400);
    let (status, v) = create(&base, "proj", Some(&["wiki", "notes", "journal"])).await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(
        enabled(&extensions_of(&base, "proj", admin).await),
        ["notes", "journal", "wiki"]
    );
    // A loca created with only a name (older clients) keeps the legacy set.
    assert_eq!(create(&base, "plain", None).await.0, 201);
    assert_eq!(
        enabled(&extensions_of(&base, "plain", admin).await),
        ["notes", "focus", "journal", "wiki", "live"]
    );
    // Chat only.
    assert_eq!(create(&base, "bare", Some(&[])).await.0, 201);
    assert!(enabled(&extensions_of(&base, "bare", admin).await).is_empty());

    // Cast: an appointed operator, a participant, a Smaster.
    let oper_key = member(&base, "oper", "user").await;
    let part_key = member(&base, "part", "user").await;
    let agent_key = member(&base, "bot", "agent").await;
    for name in ["oper", "part", "bot"] {
        seat(&base, "proj", name).await;
    }
    let oper = session(&base, ("x-room-token", &oper_key), "oper", "user").await;
    let part = session(&base, ("x-room-token", &part_key), "part", "user").await;
    let bot = session(&base, ("x-room-token", &agent_key), "bot", "agent").await;
    let (_, profiles) = call(Method::GET, format!("{base}/profiles"), admin, None).await;
    let oper_id = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "oper")
        .and_then(|p| p["id"].as_str())
        .unwrap()
        .to_string();
    let (status, v) = call(
        Method::DELETE,
        format!("{base}/rooms/proj/operators"),
        admin,
        None,
    )
    .await;
    assert!(status < 300, "{v}");
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/proj/operators"),
        admin,
        Some(serde_json::json!({ "principal_id": oper_id })),
    )
    .await;
    assert!(status < 300, "appoint: {status} {v}");
    let (_, smaster) = call(
        Method::POST,
        format!("{base}/smasters"),
        admin,
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    let deputy = session(
        &base,
        ("x-admin-token", smaster["token"].as_str().unwrap()),
        "deputy",
        "user",
    )
    .await;

    // Everyone at the table reads the set; only the operator, the Master and
    // Smasters may change it.
    let seen = extensions_of(&base, "proj", ("x-session-token", &part)).await;
    assert_eq!(seen["can_manage"], false);
    assert_eq!(
        extensions_of(&base, "proj", ("x-session-token", &oper)).await["can_manage"],
        true
    );
    for who in [&part, &bot] {
        let (status, v) = call(
            Method::PUT,
            format!("{base}/rooms/proj/extensions/focus"),
            ("x-session-token", who),
            None,
        )
        .await;
        assert_eq!(status, 403, "{v}");
        let (status, _) = call(
            Method::PUT,
            format!("{base}/rooms/proj/extensions"),
            ("x-session-token", who),
            Some(serde_json::json!({ "enabled": [] })),
        )
        .await;
        assert_eq!(status, 403);
    }
    assert_eq!(
        enabled(&extensions_of(&base, "proj", admin).await),
        ["notes", "journal", "wiki"],
        "a refused change changes nothing"
    );

    // Focus is off in proj: its reads and writes are refused with a clear
    // reason, to a participant and an agent alike.
    for who in [&part, &bot] {
        for (ext, method, path, body) in probes("proj") {
            if ext != "focus" {
                continue;
            }
            let (status, v) = call(
                method.clone(),
                format!("{base}{path}"),
                ("x-session-token", who),
                body,
            )
            .await;
            assert_eq!(status, 403, "{method} {path}: {v}");
            let text = v.as_str().unwrap_or_default();
            assert!(
                text.contains("this extension is off in this loca") && text.contains("focus"),
                "{text}"
            );
        }
    }
    // Live cannot be turned on while the live extension is off.
    let (status, v) = call(
        Method::PUT,
        format!("{base}/rooms/proj/settings"),
        ("x-session-token", &oper),
        Some(serde_json::json!({ "live": true })),
    )
    .await;
    assert_eq!(status, 403, "{v}");
    assert_eq!(
        extensions_of(&base, "proj", admin).await["enabled"]
            .as_array()
            .unwrap()
            .len(),
        3
    );

    // Notes are on: a note is written, then the operator turns notes off.
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/proj/notes"),
        ("x-session-token", &bot),
        Some(serde_json::json!({ "key": "plan", "title": "Plan", "body": "keep me", "by": "bot" })),
    )
    .await;
    assert!(status < 300, "{v}");
    let (status, v) = call(
        Method::DELETE,
        format!("{base}/rooms/proj/extensions/notes"),
        ("x-session-token", &oper),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(enabled(&v), ["journal", "wiki"]);
    assert_eq!(v["removed"], serde_json::json!(["notes"]));
    for (ext, method, path, body) in probes("proj") {
        let (status, v) = call(
            method.clone(),
            format!("{base}{path}"),
            ("x-session-token", &bot),
            body,
        )
        .await;
        if ext == "focus" || ext == "notes" {
            assert_eq!(status, 403, "{method} {path}: {v}");
        } else {
            assert!(
                !fenced(status, &v),
                "{method} {path} should not be fenced: {v}"
            );
        }
    }

    // A Smaster adds Focus and Live, the Master puts notes back: the note
    // written before is still there.
    let (status, v) = call(
        Method::PUT,
        format!("{base}/rooms/proj/extensions"),
        ("x-session-token", &deputy),
        Some(serde_json::json!({ "enabled": ["journal", "wiki", "focus", "live"] })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(enabled(&v), ["focus", "journal", "wiki", "live"]);
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/proj/extensions/notes"),
        admin,
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, note) = call(
        Method::GET,
        format!("{base}/rooms/proj/notes/plan"),
        ("x-session-token", &part),
        None,
    )
    .await;
    assert_eq!(status, 200, "{note}");
    assert_eq!(
        note["body"], "keep me",
        "removing an extension deletes nothing"
    );
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/proj/tasks"),
        ("x-session-token", &oper),
        Some(serde_json::json!({ "title": "now allowed", "by": "oper" })),
    )
    .await;
    assert!(status < 300, "focus is on again: {status} {v}");

    // Live on, then removing the live extension also ends live mode.
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/proj/settings"),
        ("x-session-token", &oper),
        Some(serde_json::json!({ "live": true })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        Method::DELETE,
        format!("{base}/rooms/proj/extensions/live"),
        ("x-session-token", &oper),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (_, settings) = call(
        Method::GET,
        format!("{base}/rooms/proj/settings"),
        admin,
        None,
    )
    .await;
    assert_eq!(settings["live"], false);

    // Unknown extension names are a 400, not a silent no-op.
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/proj/extensions/chat"),
        admin,
        None,
    )
    .await;
    assert_eq!(status, 400);

    // Every change is in the Journal: who, what (and the entry carries when).
    let lines = journal_texts(&base, "proj").await;
    let expected = [
        "extensions: opened with Notes, Journal, Loca Wiki",
        "oper | extensions: removed Notes",
        "deputy | extensions: added Focus, Live",
        "extensions: added Notes",
        "oper | extensions: removed Live",
    ];
    for want in expected {
        assert!(
            lines.iter().any(|line| line.contains(want)),
            "missing {want:?} in {lines:?}"
        );
    }
    let (_, raw) = call(
        Method::GET,
        format!("{base}/rooms/proj/journal"),
        admin,
        None,
    )
    .await;
    assert!(raw
        .as_array()
        .unwrap()
        .iter()
        .all(|e| e["at"].as_u64().unwrap() > 0));
    assert!(
        !lines.iter().any(|line| line.contains("chat")),
        "refused changes leave no Journal line"
    );

    // A closed loca's extensions do not change.
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/bare/settings"),
        admin,
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/bare/extensions/notes"),
        admin,
        None,
    )
    .await;
    assert_eq!(status, 409);
}

#[tokio::test]
async fn existing_locas_keep_every_extension_across_an_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db");
    let env = [("DB_PATH", db.to_str().unwrap().to_string())];
    let base;
    {
        let (port, guard) = spawn_server_env(MASTER, &env).await;
        base = format!("http://127.0.0.1:{port}");
        assert_eq!(create(&base, "old", None).await.0, 201);
        let (status, _) = call(
            reqwest::Method::POST,
            format!("{base}/rooms/old/notes"),
            ("x-admin-token", MASTER),
            Some(serde_json::json!({ "key": "k", "title": "t", "body": "from before", "by": "master" })),
        )
        .await;
        assert!(status < 300);
        drop(guard);
        wait_for_server_shutdown(port).await;
    }
    // Make the row look exactly like one written before extensions existed:
    // no `extensions` key in the settings JSON at all.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let raw: String = conn
            .query_row("SELECT settings FROM rooms WHERE room = 'old'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let mut json: Value = serde_json::from_str(&raw).unwrap();
        json.as_object_mut().unwrap().remove("extensions");
        conn.execute(
            "UPDATE rooms SET settings = ?1 WHERE room = 'old'",
            [json.to_string()],
        )
        .unwrap();
    }
    let (port, _guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let admin = ("x-admin-token", MASTER);
    let v = extensions_of(&base, "old", admin).await;
    assert_eq!(enabled(&v), ["notes", "focus", "journal", "wiki", "live"]);
    for (_, method, path, body) in probes("old") {
        let (status, v) = call(method.clone(), format!("{base}{path}"), admin, body).await;
        assert!(
            !fenced(status, &v),
            "{method} {path} must stay open after the upgrade: {v}"
        );
    }
    let (status, _) = call(
        reqwest::Method::PUT,
        format!("{base}/rooms/old/settings"),
        admin,
        Some(serde_json::json!({ "live": true })),
    )
    .await;
    assert_eq!(status, 200, "live still toggles in an existing loca");
    let (_, note) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/old/notes/k"),
        admin,
        None,
    )
    .await;
    assert_eq!(note["body"], "from before");
}

#[tokio::test]
async fn a_birebir_has_no_extensions() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("bb-ext.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let ayse_key = member(&base, "ayse", "user").await;
    let mehmet_key = member(&base, "mehmet", "user").await;
    assert_eq!(create(&base, "team", None).await.0, 201);
    seat(&base, "team", "ayse").await;
    seat(&base, "team", "mehmet").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let (status, opened) = call(
        reqwest::Method::POST,
        format!("{base}/birebirs"),
        ("x-session-token", &ayse),
        Some(serde_json::json!({ "name": "mehmet" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let room = opened["birebir"]["room"].as_str().unwrap().to_string();

    let v = extensions_of(&base, &room, ("x-session-token", &ayse)).await;
    assert!(enabled(&v).is_empty());
    assert_eq!(v["birebir"], true);
    assert_eq!(v["can_manage"], false);
    // Nobody can give a birebir an extension: not a party, not the Master.
    for header in [
        ("x-session-token", ayse.as_str()),
        ("x-admin-token", MASTER),
    ] {
        for (method, path, body) in [
            (
                reqwest::Method::PUT,
                format!("/rooms/{room}/extensions/notes"),
                None,
            ),
            (
                reqwest::Method::PUT,
                format!("/rooms/{room}/extensions"),
                Some(serde_json::json!({ "enabled": ["notes"] })),
            ),
        ] {
            let (status, v) = call(method, format!("{base}{path}"), header, body).await;
            assert!(status == 403 || status == 404, "{path}: {status} {v}");
        }
    }
    // A loca cannot be created under the birebir prefix to sneak one in.
    assert_eq!(create(&base, "bb-fake", Some(&["notes"])).await.0, 409);
    // Its extension routes answer 403 to the two.
    for (_, method, path, body) in probes(&room) {
        let (status, _) = call(
            method.clone(),
            format!("{base}{path}"),
            ("x-session-token", &ayse),
            body,
        )
        .await;
        assert_eq!(status, 403, "{method} {path}");
    }
    let _ = mehmet_key;
}

#[tokio::test]
async fn search_returns_no_notes_while_the_notes_extension_is_off() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("ext-search.db");
    let (port, _guard) =
        spawn_server_env(MASTER, &[("DB_PATH", db.to_str().unwrap().into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let admin = ("x-admin-token", MASTER);
    // Notes on: a note is written and found.
    assert!(create(&base, "ext-search", Some(&["notes"])).await.0 < 300);
    let (status, _) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/ext-search/notes"),
        admin,
        Some(serde_json::json!({ "key": "plan", "body": "the secret plan", "by": "operator" })),
    )
    .await;
    assert!(status < 300, "note write: {status}");
    let (_, found) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/ext-search/search?q=secret"),
        admin,
        None,
    )
    .await;
    assert_eq!(found["notes"].as_array().unwrap().len(), 1, "{found}");
    // Notes off: search no longer reaches them (records are kept).
    let (status, _) = call(
        reqwest::Method::PUT,
        format!("{base}/rooms/ext-search/extensions"),
        admin,
        Some(serde_json::json!({ "enabled": [] })),
    )
    .await;
    assert!(status < 300);
    let (_, found) = call(
        reqwest::Method::GET,
        format!("{base}/rooms/ext-search/search?q=secret"),
        admin,
        None,
    )
    .await;
    assert!(found["notes"].as_array().unwrap().is_empty(), "{found}");
}

/// Changing a loca's extensions and recording it in the Journal are one
/// write. When the Journal line cannot be written the change does not take
/// effect — in the running server, on disk, or after a restart — and the
/// same change succeeds once the Journal can be written again.
#[tokio::test]
async fn a_failed_journal_line_leaves_the_extensions_unchanged() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("extension-atomic.db");
    let env = [("DB_PATH", db.to_str().unwrap().to_string())];
    let admin = ("x-admin-token", MASTER);
    let stored = |db: &std::path::Path| -> Vec<String> {
        let sql = rusqlite::Connection::open(db).unwrap();
        let settings: String = sql
            .query_row(
                "SELECT settings FROM rooms WHERE room = 'atomic-ext'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let settings: Value = serde_json::from_str(&settings).unwrap();
        settings["extensions"]
            .as_array()
            .expect("stored extension set")
            .iter()
            .map(|id| id.as_str().unwrap().to_string())
            .collect()
    };

    let (port, guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        create(&base, "atomic-ext", Some(&["notes", "journal"]))
            .await
            .0,
        201
    );
    let before = enabled(&extensions_of(&base, "atomic-ext", admin).await);
    let lines_before = journal_texts(&base, "atomic-ext").await;
    assert_eq!(stored(&db), before);

    // The Journal line that would record the change cannot be written.
    let sql = rusqlite::Connection::open(&db).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_journal BEFORE INSERT ON journal
         BEGIN SELECT RAISE(ABORT, 'injected journal failure'); END;",
    )
    .unwrap();
    let wanted = serde_json::json!({ "enabled": ["notes", "journal", "focus"] });
    let (status, _) = call(
        Method::PUT,
        format!("{base}/rooms/atomic-ext/extensions"),
        admin,
        Some(wanted.clone()),
    )
    .await;
    assert_eq!(status, 503, "the failed write is reported");
    assert_eq!(
        enabled(&extensions_of(&base, "atomic-ext", admin).await),
        before,
        "the running server did not change"
    );
    assert_eq!(
        call(
            Method::GET,
            format!("{base}/rooms/atomic-ext/tasks"),
            admin,
            None
        )
        .await
        .0,
        403,
        "Focus is still off, so its records stay refused"
    );
    assert_eq!(stored(&db), before, "the stored setting did not change");
    assert_eq!(journal_texts(&base, "atomic-ext").await, lines_before);

    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        enabled(&extensions_of(&base, "atomic-ext", admin).await),
        before,
        "the same after a restart"
    );
    assert_eq!(journal_texts(&base, "atomic-ext").await, lines_before);

    // With the Journal writable again the same change lands whole.
    sql.execute_batch("DROP TRIGGER fail_journal;").unwrap();
    drop(sql);
    let (status, v) = call(
        Method::PUT,
        format!("{base}/rooms/atomic-ext/extensions"),
        admin,
        Some(wanted),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let after = enabled(&extensions_of(&base, "atomic-ext", admin).await);
    assert!(after.contains(&"focus".to_string()), "{after:?}");
    assert_eq!(stored(&db), after);
    let lines_after = journal_texts(&base, "atomic-ext").await;
    assert_eq!(
        lines_after.len(),
        lines_before.len() + 1,
        "exactly one Journal line records the change: {lines_after:?}"
    );

    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        enabled(&extensions_of(&base, "atomic-ext", admin).await),
        after
    );
    assert_eq!(journal_texts(&base, "atomic-ext").await, lines_after);
}
