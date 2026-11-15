//! A rank's rename relabels the messages that person wrote, and only those:
//! what a connected client is told, what a fresh client reads, and what a
//! restarted server serves.
use super::*;

const ROOT: &str = "rename-fixture-root";
const ROOM: &str = "rename-own";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn boot(db: &str, sessions_required: bool) -> (u16, ServerGuard) {
    let mut env = vec![("DB_PATH", db.to_string()), ("REQUIRE_INVITE", "1".into())];
    if sessions_required {
        env.push(("REQUIRE_SESSIONS", "1".into()));
    }
    spawn_server_env(ROOT, &env).await
}

async fn call(base: &str, method: &str, path: &str, header: (&str, &str), body: Value) -> Value {
    let response = reqwest::Client::new()
        .request(method.parse().unwrap(), format!("{base}{path}"))
        .header(header.0, header.1)
        .json(&body)
        .send()
        .await
        .unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "{method} {path}: {status} {text}");
    serde_json::from_str(&text).unwrap_or(Value::Null)
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    call(
        base,
        "POST",
        "/sessions",
        header,
        serde_json::json!({ "name": name, "kind": kind }),
    )
    .await["session_token"]
        .as_str()
        .unwrap()
        .to_string()
}

async fn say(
    base: &str,
    session: &str,
    text: &str,
    reply_to: Option<u64>,
    op: Option<&str>,
) -> u64 {
    say_in(base, ROOM, session, text, reply_to, op).await
}

async fn say_in(
    base: &str,
    room: &str,
    session: &str,
    text: &str,
    reply_to: Option<u64>,
    op: Option<&str>,
) -> u64 {
    call(
        base,
        "POST",
        &format!("/rooms/{room}/messages"),
        ("x-session-token", session),
        serde_json::json!({
            "sender": "ignored", "sender_type": "user", "text": text,
            "reply_to": reply_to, "op_id": op,
        }),
    )
    .await["id"]
        .as_u64()
        .unwrap()
}

async fn ws_with(port: u16, protocol: String, name: &str, watch: bool) -> Ws {
    ws_in(port, ROOM, protocol, &format!("name={name}"), watch).await
}

async fn ws_in(port: u16, room: &str, protocol: String, query: &str, watch: bool) -> Ws {
    let watch = if watch { "&watch=1" } else { "" };
    connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&{query}{watch}"),
        &["loca.v1".to_string(), protocol],
    )
    .await
}

/// The live tail (`GET` without `since`) and the archive (`since=0`).
async fn tail_and_archive(base: &str) -> (Vec<Value>, Vec<Value>) {
    let get = |path: String| async move {
        reqwest::Client::new()
            .get(format!("{base}{path}"))
            .header("x-admin-token", ROOT)
            .send()
            .await
            .unwrap()
            .json::<Vec<Value>>()
            .await
            .unwrap()
    };
    (
        get(format!("/rooms/{ROOM}/messages")).await,
        get(format!("/rooms/{ROOM}/messages?since=0&limit=1000")).await,
    )
}

fn field(messages: &[Value], id: u64, key: &str) -> Value {
    messages
        .iter()
        .find(|m| m["id"] == id)
        .unwrap_or_else(|| panic!("message {id} missing"))[key]
        .clone()
}

#[tokio::test]
async fn a_rename_reaches_only_its_own_messages_live_and_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("rename.db").to_string_lossy().to_string();

    // Boot 1: a loca, a Smaster, and a member who first wears "shared".
    let (port, guard) = boot(&db, true).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, ROOM).await;
    let deputy_code = call(
        &base,
        "POST",
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "deputy-start" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let member_token = call(
        &base,
        "POST",
        "/members",
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "shared", "kind": "user" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    call(
        &base,
        "POST",
        &format!("/rooms/{ROOM}/invites"),
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "shared" }),
    )
    .await;
    drop(guard);
    wait_for_server_shutdown(port).await;

    // A line written under "shared" before writers were recorded.
    let legacy_id = {
        let sql = rusqlite::Connection::open(&db).unwrap();
        sql.busy_timeout(Duration::from_secs(5)).unwrap();
        let id: u64 = sql
            .query_row("SELECT COALESCE(MAX(id), 0) + 1 FROM messages", [], |row| {
                row.get(0)
            })
            .unwrap();
        sql.execute(
            "INSERT INTO messages (id, room, sender, sender_type, text, ts, kind)
             VALUES (?1, ?2, 'shared', 'user', 'legacy words', 1, 'say')",
            rusqlite::params![id, ROOM],
        )
        .unwrap();
        id
    };

    // Boot 2.
    let (port, guard) = boot(&db, true).await;
    let base = format!("http://127.0.0.1:{port}");
    let member = session(&base, ("x-room-token", &member_token), "shared", "user").await;
    let member_words = say(&base, &member, "member words", None, Some("op-member")).await;
    call(
        &base,
        "DELETE",
        &format!("/members/{}", member_ref(&member_token)),
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;

    // The Smaster takes the freed label and speaks over REST and the socket.
    let deputy = session(
        &base,
        ("x-admin-token", &deputy_code),
        "deputy-start",
        "user",
    )
    .await;
    call(
        &base,
        "PUT",
        "/profile",
        ("x-session-token", &deputy),
        serde_json::json!({ "display_name": "shared" }),
    )
    .await;
    let rest_words = say(&base, &deputy, "smaster rest words", None, Some("op-sm")).await;
    assert_eq!(
        say(&base, &deputy, "smaster rest words", None, Some("op-sm")).await,
        rest_words,
        "an operation replay still returns the first message"
    );
    let bare_words = say(&base, &deputy, "smaster words without op id", None, None).await;
    let mut deputy_ws = ws_with(port, format!("loca.session.{deputy}"), "shared", false).await;
    wait_for(&mut deputy_ws, |v| v["t"] == "history").await;
    deputy_ws
        .send(WsMessage::Text(
            serde_json::json!({ "t": "send", "text": "smaster socket words" }).to_string(),
        ))
        .await
        .unwrap();
    let socket_words = wait_for(&mut deputy_ws, |v| {
        v["t"] == "msg" && v["message"]["text"] == "smaster socket words"
    })
    .await["message"]["id"]
        .as_u64()
        .unwrap();

    // Someone answers each of the three writers who wore "shared".
    let helper_invite = invite_for(&base, ROOT, ROOM, "helper").await;
    let helper = session(&base, ("x-room-token", &helper_invite), "helper", "agent").await;
    let to_member = say(&base, &helper, "to the member", Some(member_words), None).await;
    let to_legacy = say(&base, &helper, "to the legacy line", Some(legacy_id), None).await;
    let to_smaster = say(&base, &helper, "to the smaster", Some(rest_words), None).await;

    // A connected watcher sees the rename.
    let master = session(&base, ("x-admin-token", ROOT), "watcher", "user").await;
    let mut watcher = ws_with(port, format!("loca.session.{master}"), "watcher", true).await;
    wait_for(&mut watcher, |v| v["t"] == "history").await;
    call(
        &base,
        "PUT",
        "/profile",
        ("x-session-token", &deputy),
        serde_json::json!({ "display_name": "renamed" }),
    )
    .await;
    // The id frame comes first, then the by-name frame older pages know,
    // in exactly its old shape.
    let mut told = Vec::new();
    while told
        .last()
        .map(|v: &Value| v["t"] != "renamed")
        .unwrap_or(true)
    {
        told.push(
            wait_for(&mut watcher, |v| {
                v["t"] == "relabelled" || v["t"] == "renamed"
            })
            .await,
        );
    }
    assert_eq!(told.len(), 2, "{told:?}");
    assert_eq!(
        told[1],
        serde_json::json!({ "t": "renamed", "from": "shared", "to": "renamed" })
    );
    let told = &told[0];
    assert_eq!(told["t"], "relabelled");
    assert_eq!(told["to"], "renamed");
    let mut ids: Vec<u64> = told["ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_u64().unwrap())
        .collect();
    ids.sort_unstable();
    assert_eq!(ids, vec![rest_words, bare_words, socket_words]);
    assert_eq!(told["reply_ids"], serde_json::json!([to_smaster]));

    let mine = [rest_words, bare_words, socket_words];
    let others = [legacy_id, member_words];
    let check = |messages: &[Value], replies_labelled: bool| {
        for id in mine {
            assert_eq!(field(messages, id, "sender"), "renamed", "message {id}");
        }
        for id in others {
            assert_eq!(field(messages, id, "sender"), "shared", "message {id}");
        }
        let label = |id| field(messages, id, "reply_to_sender");
        if replies_labelled {
            assert_eq!(label(to_smaster), "renamed");
            assert_eq!(label(to_member), "shared");
            assert_eq!(label(to_legacy), "shared");
        } else {
            for id in [to_smaster, to_member, to_legacy] {
                assert_eq!(label(id), Value::Null, "reply labels are not stored");
            }
        }
    };
    let (tail, archive) = tail_and_archive(&base).await;
    check(&tail, true);
    check(&archive, false);
    let mut fresh = ws_with(port, format!("loca.session.{master}"), "fresh", true).await;
    let history = wait_for(&mut fresh, |v| v["t"] == "history").await;
    check(history["messages"].as_array().unwrap(), true);
    drop((fresh, watcher, deputy_ws, guard));
    wait_for_server_shutdown(port).await;

    // Boot 3: the restarted server serves the same labels.
    let (port, guard) = boot(&db, true).await;
    let base = format!("http://127.0.0.1:{port}");
    let (tail, archive) = tail_and_archive(&base).await;
    check(&tail, false);
    check(&archive, false);
    let master = session(&base, ("x-admin-token", ROOT), "watcher", "user").await;
    let mut fresh = ws_with(port, format!("loca.session.{master}"), "fresh", true).await;
    let history = wait_for(&mut fresh, |v| v["t"] == "history").await;
    check(history["messages"].as_array().unwrap(), false);
    drop((fresh, guard));
    wait_for_server_shutdown(port).await;

    // Storage: one writer for the Smaster's lines, a different one for the
    // member's, none for the legacy line; the operation scope is the
    // Smaster's principal, which the rename did not change.
    let sql = rusqlite::Connection::open(&db).unwrap();
    let writer = |id: u64| -> Option<String> {
        sql.query_row(
            "SELECT sender_principal FROM messages WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    };
    let deputy_principal = writer(rest_words).expect("REST writer recorded");
    assert_eq!(
        writer(bare_words).as_deref(),
        Some(deputy_principal.as_str())
    );
    assert_eq!(
        writer(socket_words).as_deref(),
        Some(deputy_principal.as_str())
    );
    let member_principal = writer(member_words).expect("member writer recorded");
    assert_ne!(member_principal, deputy_principal);
    assert!(member_principal.starts_with("pr_") && deputy_principal.starts_with("pr_"));
    assert_eq!(writer(legacy_id), None);
    let scope: Option<String> = sql
        .query_row(
            "SELECT principal FROM messages WHERE id = ?1",
            [rest_words],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(scope, Some(format!("pr:{deputy_principal}")));
}

/// A Smaster that speaks over the socket with its code (no session) is the
/// seat's principal; the root key names nobody.
#[tokio::test]
async fn a_smaster_code_socket_records_its_principal_and_the_root_key_records_none() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("code.db").to_string_lossy().to_string();
    let (port, guard) = boot(&db, false).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, ROOM).await;
    let deputy_code = call(
        &base,
        "POST",
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "deputy" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let mut sent = Vec::new();
    for (protocol, text) in [
        (format!("loca.admin.{deputy_code}"), "with the code"),
        (format!("loca.admin.{ROOT}"), "with the root key"),
    ] {
        let mut ws = ws_with(port, protocol, "deputy", false).await;
        wait_for(&mut ws, |v| v["t"] == "history").await;
        ws.send(WsMessage::Text(
            serde_json::json!({ "t": "send", "text": text }).to_string(),
        ))
        .await
        .unwrap();
        let id = wait_for(&mut ws, |v| v["t"] == "msg" && v["message"]["text"] == text).await
            ["message"]["id"]
            .as_u64()
            .unwrap();
        sent.push(id);
    }
    drop(guard);
    wait_for_server_shutdown(port).await;
    let sql = rusqlite::Connection::open(&db).unwrap();
    let writer = |id: u64| -> Option<String> {
        sql.query_row(
            "SELECT sender_principal FROM messages WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .unwrap()
    };
    let deputy_principal: String = sql
        .query_row("SELECT principal_id FROM smasters", [], |row| row.get(0))
        .unwrap();
    assert_eq!(writer(sent[0]), Some(deputy_principal));
    assert_eq!(writer(sent[1]), None);
}

/// Read every frame up to the sentinel message `text`; return the rename
/// frames among them, in order.
async fn rename_frames_before(ws: &mut Ws, text: &str) -> Vec<Value> {
    let mut seen = Vec::new();
    loop {
        let frame = wait_for(ws, |_| true).await;
        if frame["t"] == "msg" && frame["message"]["text"] == text {
            return seen;
        }
        if frame["t"] == "renamed" || frame["t"] == "relabelled" {
            seen.push(frame);
        }
    }
}

/// `Renamed {from, to}` is sent in exactly the rooms where the previous
/// server sent it (a person's message under the old name in the in-memory
/// tail), never more; the id frame carries what changed; a `filter=msg`
/// socket, which is how the agent kit listens, sees neither.
#[tokio::test]
async fn the_by_name_frame_is_sent_exactly_where_it_was_before() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("frames.db").to_string_lossy().to_string();
    let (port, _guard) = boot(&db, true).await;
    let base = format!("http://127.0.0.1:{port}");
    // mine: the renamer speaks; tail: its words and a former wearer's words
    // have both left the tail, answers to each are in it; former: only a
    // former wearer's words are in the tail.
    let (mine, tail, former) = ("rename-mine", "rename-tail", "rename-former");
    for room in [mine, tail, former] {
        create_test_room(port, ROOT, room).await;
    }
    let deputy_code = call(
        &base,
        "POST",
        "/smasters",
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "deputy-start" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    let member_token = call(
        &base,
        "POST",
        "/members",
        ("x-admin-token", ROOT),
        serde_json::json!({ "name": "shared", "kind": "user" }),
    )
    .await["token"]
        .as_str()
        .unwrap()
        .to_string();
    for room in [tail, former] {
        call(
            &base,
            "POST",
            &format!("/rooms/{room}/invites"),
            ("x-admin-token", ROOT),
            serde_json::json!({ "name": "shared" }),
        )
        .await;
    }
    let member = session(&base, ("x-room-token", &member_token), "shared", "user").await;
    let member_in_tail = say_in(&base, tail, &member, "member, long ago", None, None).await;
    say_in(&base, former, &member, "member, still in view", None, None).await;
    call(
        &base,
        "DELETE",
        &format!("/members/{}", member_ref(&member_token)),
        ("x-admin-token", ROOT),
        Value::Null,
    )
    .await;

    let deputy = session(
        &base,
        ("x-admin-token", &deputy_code),
        "deputy-start",
        "user",
    )
    .await;
    call(
        &base,
        "PUT",
        "/profile",
        ("x-session-token", &deputy),
        serde_json::json!({ "display_name": "shared" }),
    )
    .await;
    let mine_words = say_in(&base, mine, &deputy, "deputy, in view", None, None).await;
    let deputy_in_tail = say_in(&base, tail, &deputy, "deputy, long ago", None, None).await;

    // Push both "shared" lines in `tail` out of the in-memory history, then
    // answer each of them from an agent.
    let master = session(&base, ("x-admin-token", ROOT), "watcher", "user").await;
    for n in 0..200 {
        say_in(&base, tail, &master, &format!("filler {n}"), None, None).await;
    }
    let helper_invite = invite_for(&base, ROOT, tail, "helper").await;
    let helper = session(&base, ("x-room-token", &helper_invite), "helper", "agent").await;
    let to_member = say_in(
        &base,
        tail,
        &helper,
        "to the member",
        Some(member_in_tail),
        None,
    )
    .await;
    let to_deputy = say_in(
        &base,
        tail,
        &helper,
        "to the deputy",
        Some(deputy_in_tail),
        None,
    )
    .await;
    let recent: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/{tail}/messages"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(recent
        .iter()
        .all(|m| m["id"] != member_in_tail && m["id"] != deputy_in_tail));
    assert!(recent.iter().all(|m| m["sender"] != "shared"));

    let mut watchers = Vec::new();
    for room in [mine, tail, former] {
        let mut ws = ws_in(
            port,
            room,
            format!("loca.session.{master}"),
            "name=watcher",
            true,
        )
        .await;
        wait_for(&mut ws, |v| v["t"] == "history").await;
        watchers.push(ws);
    }
    let mut kit = ws_in(
        port,
        tail,
        format!("loca.session.{helper}"),
        "name=helper&type=agent&filter=msg",
        false,
    )
    .await;
    // A filter=msg socket gets no history; its memory frame is sent only once
    // it is seated. Without it the rename below can precede the socket and
    // its "sees neither frame" proves nothing (or it misses the sentinel).
    let _ = wait_for(&mut kit, |v| v["t"] == "memory").await;

    call(
        &base,
        "PUT",
        "/profile",
        ("x-session-token", &deputy),
        serde_json::json!({ "display_name": "renamed" }),
    )
    .await;
    for room in [mine, tail, former] {
        say_in(&base, room, &master, "sentinel", None, None).await;
    }
    let by_name = serde_json::json!({ "t": "renamed", "from": "shared", "to": "renamed" });
    let mut frames = Vec::new();
    for ws in watchers.iter_mut() {
        frames.push(rename_frames_before(ws, "sentinel").await);
    }
    assert_eq!(
        frames[0],
        vec![
            serde_json::json!({ "t": "relabelled", "to": "renamed", "ids": [mine_words], "reply_ids": [] }),
            by_name.clone(),
        ],
        "the renamer's words are in view: told by id, then by name as before"
    );
    assert_eq!(
        frames[1],
        vec![
            serde_json::json!({ "t": "relabelled", "to": "renamed", "ids": [], "reply_ids": [to_deputy] })
        ],
        "nobody's words under the old name are in view: no by-name frame"
    );
    assert_eq!(
        frames[2],
        vec![by_name],
        "a former wearer's words are in view: told by name as before, nothing by id"
    );
    assert!(
        rename_frames_before(&mut kit, "sentinel").await.is_empty(),
        "a filter=msg socket sees neither frame"
    );

    let recent: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/{tail}/messages"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(field(&recent, to_deputy, "reply_to_sender"), "renamed");
    assert_eq!(field(&recent, to_member, "reply_to_sender"), "shared");
}
