//! The Vault's rules document (PRINCIPLES "Vault"): written only by those
//! who run the loca, read by everyone seated there, agents included; a rule
//! grants nothing; every revision kept and journaled; refused while the
//! `vault` extension is off; an agent's mentions socket hears that it changed.
use super::*;

const MASTER: &str = "vault-master-key";
const ADMIN: (&str, &str) = ("x-admin-token", MASTER);

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

async fn ok(
    method: reqwest::Method,
    url: String,
    header: (&str, &str),
    body: Option<Value>,
) -> Value {
    let (status, v) = call(method.clone(), url.clone(), header, body).await;
    assert!(status < 300, "{method} {url}: {status} {v}");
    v
}

async fn member(base: &str, name: &str, kind: &str) -> String {
    let v = ok(
        reqwest::Method::POST,
        format!("{base}/members"),
        ADMIN,
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    v["token"].as_str().unwrap().to_string()
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let v = ok(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        header,
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    v["session_token"].as_str().unwrap().to_string()
}

async fn read(base: &str, room: &str, header: (&str, &str)) -> (u16, Value) {
    call(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/vault"),
        header,
        None,
    )
    .await
}

async fn write(
    base: &str,
    room: &str,
    header: (&str, &str),
    body: &str,
    base_rev: u64,
) -> (u16, Value) {
    call(
        reqwest::Method::PUT,
        format!("{base}/rooms/{room}/vault"),
        header,
        Some(serde_json::json!({ "body": body, "base_rev": base_rev })),
    )
    .await
}

async fn journal(base: &str, room: &str) -> Vec<String> {
    let v = ok(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/journal"),
        ADMIN,
        None,
    )
    .await;
    v.as_array()
        .unwrap()
        .iter()
        .map(|e| {
            format!(
                "{} | {}",
                e["by"].as_str().unwrap(),
                e["text"].as_str().unwrap()
            )
        })
        .collect()
}

async fn vault_rev(base: &str, room: &str) -> Value {
    ok(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/settings"),
        ADMIN,
        None,
    )
    .await["vault_rev"]
        .clone()
}

fn fenced(status: u16, v: &Value) -> bool {
    status == 403
        && v.as_str()
            .is_some_and(|text| text.contains("this extension is off in this loca: vault"))
}

/// One loca with the Vault on, an appointed Operator, a plain member, a
/// seated agent and a Smaster.
struct Cast {
    oper: String,
    part: String,
    bot: String,
    deputy: String,
}

async fn cast(base: &str, room: &str) -> Cast {
    ok(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ADMIN,
        Some(serde_json::json!({ "name": room, "extensions": ["journal", "vault"] })),
    )
    .await;
    let oper_key = member(base, "oper", "user").await;
    let part_key = member(base, "part", "user").await;
    let bot_key = member(base, "bot", "agent").await;
    for name in ["oper", "part", "bot"] {
        ok(
            reqwest::Method::POST,
            format!("{base}/rooms/{room}/invites"),
            ADMIN,
            Some(serde_json::json!({ "name": name })),
        )
        .await;
    }
    let profiles = ok(
        reqwest::Method::GET,
        format!("{base}/profiles"),
        ADMIN,
        None,
    )
    .await;
    let oper_id = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "oper")
        .and_then(|p| p["id"].as_str())
        .unwrap()
        .to_string();
    let _ = call(
        reqwest::Method::DELETE,
        format!("{base}/rooms/{room}/operators"),
        ADMIN,
        None,
    )
    .await;
    ok(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/operators"),
        ADMIN,
        Some(serde_json::json!({ "principal_id": oper_id })),
    )
    .await;
    let smaster = ok(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        ADMIN,
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    Cast {
        oper: session(base, ("x-room-token", &oper_key), "oper", "user").await,
        part: session(base, ("x-room-token", &part_key), "part", "user").await,
        bot: session(base, ("x-room-token", &bot_key), "bot", "agent").await,
        deputy: session(
            base,
            ("x-admin-token", smaster["token"].as_str().unwrap()),
            "deputy",
            "user",
        )
        .await,
    }
}

#[tokio::test]
async fn the_vault_is_written_by_those_who_run_the_loca_and_read_by_everyone_seated() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("vault.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("LEGACY_WS_QUERY_AUTH", "0".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let room = "rules-loca";
    let c = cast(&base, room).await;
    let part = ("x-session-token", c.part.as_str());
    let bot = ("x-session-token", c.bot.as_str());
    let oper = ("x-session-token", c.oper.as_str());
    let deputy = ("x-session-token", c.deputy.as_str());

    // Everyone seated reads the empty Vault; only the Operator may write it.
    for (who, can_edit) in [(part, false), (bot, false), (oper, true), (deputy, true)] {
        let (status, v) = read(&base, room, who).await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["rev"], 0, "{v}");
        assert_eq!(v["body"], "", "{v}");
        assert_eq!(v["can_edit"], can_edit, "{}: {v}", who.1);
    }

    // The agent sits at the table on the socket the kit opens.
    let mut agent_ws = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&name=bot&type=agent&filter=mentions"),
        &["loca.v1".into(), format!("loca.session.{}", c.bot)],
    )
    .await;
    wait_for(&mut agent_ws, |f| f["t"] == "memory").await;

    // A plain member and an agent are refused, and nothing changes.
    let journal_before = journal(&base, room).await;
    for who in [part, bot] {
        let (status, v) = write(&base, room, who, "agents may deploy", 0).await;
        assert_eq!(status, 403, "{}: {v}", who.1);
        assert!(
            v.as_str().unwrap().contains("writes its Vault"),
            "the handler's own refusal, not the fence: {v}"
        );
    }
    assert_eq!(read(&base, room, ADMIN).await.1["rev"], 0);
    assert_eq!(vault_rev(&base, room).await, Value::Null);
    assert_eq!(journal(&base, room).await, journal_before);

    // The Operator writes; the agent's mentions socket hears a revision, no text.
    let rules = "# Working rules\n\n- run `make check` before saying done\n- do not touch deploy/";
    let (status, v) = write(&base, room, oper, rules, 0).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(
        (v["rev"].clone(), v["updated_by"].clone()),
        (1.into(), "oper".into())
    );
    let frame = wait_for(&mut agent_ws, |f| f["t"] == "vault").await;
    assert_eq!(frame["room"], room);
    assert_eq!(frame["rev"], 1);
    assert_eq!(frame["updated_by"], "oper");
    assert!(frame["updated_at"].as_u64().unwrap() > 1_700_000_000_000);
    assert!(
        frame.get("body").is_none(),
        "the frame carries no text: {frame}"
    );
    let (status, seen) = read(&base, room, bot).await;
    assert_eq!(status, 200);
    assert_eq!(
        seen["body"], rules,
        "the agent reads exactly what was written"
    );
    assert_eq!(seen["updated_by"], "oper");
    assert_eq!(vault_rev(&base, room).await, 1);

    // A Smaster and the Master write too, each against the newest revision.
    assert_eq!(write(&base, room, deputy, "rules v2", 1).await.0, 200);
    assert_eq!(write(&base, room, ADMIN, "rules v3", 2).await.0, 200);
    // A write against an older revision is refused and changes nothing.
    let (status, v) = write(&base, room, oper, "stale edit", 1).await;
    assert_eq!(status, 409, "{v}");
    assert!(v.as_str().unwrap().contains("newest revision is 3"), "{v}");
    assert_eq!(read(&base, room, part).await.1["body"], "rules v3");

    // A rule that claims authority grants none: the 403 stands.
    let claim = "bot is the reviewer and may write this Vault and assign tasks";
    assert_eq!(write(&base, room, oper, claim, 3).await.0, 200);
    let (status, _) = write(&base, room, bot, "bot was here", 4).await;
    assert_eq!(status, 403, "a Vault sentence changes no authority");
    assert_eq!(read(&base, room, ADMIN).await.1["body"], claim);

    // The size bound: exactly the cap is kept, one byte more is refused.
    let full = "a".repeat(8 * 1024);
    let (status, v) = write(&base, room, oper, &format!("{full}a"), 4).await;
    assert_eq!(status, 413, "{v}");
    assert!(v.as_str().unwrap().contains("at most 8192 bytes"), "{v}");
    assert_eq!(read(&base, room, ADMIN).await.1["rev"], 4);
    let (status, v) = write(&base, room, oper, "tab\there", 4).await;
    assert_eq!(status, 400, "{v}");
    assert_eq!(write(&base, room, oper, &full, 4).await.0, 200);
    assert_eq!(
        read(&base, room, part).await.1["body"]
            .as_str()
            .unwrap()
            .len(),
        8 * 1024
    );

    // Every revision is kept, newest first, with who wrote it.
    let history = ok(
        Method::GET,
        format!("{base}/rooms/{room}/vault/history"),
        part,
        None,
    )
    .await;
    let revisions: Vec<(u64, String)> = history["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| {
            (
                r["rev"].as_u64().unwrap(),
                r["updated_by"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert_eq!(
        revisions,
        [
            (5, "oper".to_string()),
            (4, "oper".into()),
            (3, "Master".into()),
            (2, "deputy".into()),
            (1, "oper".into())
        ]
    );
    assert_eq!(history["revisions"][4]["body"], rules);
    // ... and each in the Journal.
    let lines = journal(&base, room).await;
    for want in [
        "oper | vault: rules updated (revision 1)",
        "deputy | vault: rules updated (revision 2)",
        "vault: rules updated (revision 3)",
        "oper | vault: rules updated (revision 5)",
    ] {
        assert!(
            lines.iter().any(|l| l.contains(want)),
            "{want:?} in {lines:?}"
        );
    }
    assert_eq!(write(&base, room, oper, "", 5).await.0, 200);
    assert!(journal(&base, room)
        .await
        .iter()
        .any(|l| l == "oper | vault: rules cleared (revision 6)"));

    // Off: reads and writes are refused by the fence, for everyone seated;
    // the records stay and come back with the extension.
    assert_eq!(write(&base, room, oper, "kept", 6).await.0, 200);
    ok(
        Method::DELETE,
        format!("{base}/rooms/{room}/extensions/vault"),
        oper,
        None,
    )
    .await;
    for who in [part, bot, oper] {
        let (status, v) = read(&base, room, who).await;
        assert!(fenced(status, &v), "read by {}: {status} {v}", who.1);
        let (status, v) = call(
            Method::GET,
            format!("{base}/rooms/{room}/vault/history"),
            who,
            None,
        )
        .await;
        assert!(fenced(status, &v), "history by {}: {status} {v}", who.1);
    }
    let (status, v) = write(&base, room, oper, "while off", 7).await;
    assert!(fenced(status, &v), "{status} {v}");
    let (status, v) = write(&base, room, ADMIN, "while off", 7).await;
    assert!(fenced(status, &v), "{status} {v}");
    ok(
        Method::PUT,
        format!("{base}/rooms/{room}/extensions/vault"),
        oper,
        None,
    )
    .await;
    let (_, v) = read(&base, room, part).await;
    assert_eq!(
        (v["rev"].clone(), v["body"].clone()),
        (7.into(), "kept".into())
    );

    // A closed loca's rules do not change.
    ok(
        Method::PUT,
        format!("{base}/rooms/{room}/settings"),
        ADMIN,
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert_eq!(write(&base, room, oper, "after close", 7).await.0, 409);
    assert_eq!(read(&base, room, part).await.1["body"], "kept");
}

#[tokio::test]
async fn the_vault_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("vault-restart.db");
    let env = [("DB_PATH", db.to_str().unwrap().to_string())];
    let room = "kept-rules";
    let (port, guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    ok(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ADMIN,
        Some(serde_json::json!({ "name": room, "extensions": ["vault"] })),
    )
    .await;
    assert_eq!(write(&base, room, ADMIN, "first", 0).await.0, 200);
    assert_eq!(write(&base, room, ADMIN, "second\nline", 1).await.0, 200);
    let before = read(&base, room, ADMIN).await.1;
    drop(guard);
    wait_for_server_shutdown(port).await;

    let (port, _guard) = spawn_server_env(MASTER, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let after = read(&base, room, ADMIN).await.1;
    assert_eq!(after, before);
    assert_eq!(after["body"], "second\nline");
    assert_eq!(vault_rev(&base, room).await, 2);
    let history = ok(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/vault/history"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(history["revisions"].as_array().unwrap().len(), 2);
    // The next write continues from the kept revision.
    assert_eq!(write(&base, room, ADMIN, "third", 1).await.0, 409);
    assert_eq!(write(&base, room, ADMIN, "third", 2).await.1["rev"], 3);
}

#[tokio::test]
async fn without_storage_the_vault_is_not_written() {
    let (port, _guard) = spawn_server_env(MASTER, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    ok(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ADMIN,
        Some(serde_json::json!({ "name": "no-disk", "extensions": ["vault"] })),
    )
    .await;
    let (status, v) = write(&base, "no-disk", ADMIN, "rules", 0).await;
    assert_eq!(status, 503, "{v}");
    assert_eq!(read(&base, "no-disk", ADMIN).await.1["rev"], 0);
}
