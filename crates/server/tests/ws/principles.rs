//! Three promises of PRINCIPLES.md, pinned against the real binary so that a
//! later change cannot break one of them silently:
//!
//! - "Conversation is conversation" (Four principles, 3): speech does not
//!   create or change a work record; only an explicit action does.
//! - "A word spoken to you must not disappear" (the reliability paragraph
//!   after the spirit test): an acknowledged message survives a restart, a
//!   re-send of the same operation lands once, and an absent reader finds it.
//! - "Loca extensions › Boundaries": an extension that is off is not merely
//!   hidden; the server refuses it.
//!
//! Where today's behaviour falls short of the principle, the assertion is
//! guarded by a `KNOWN_GAP_*` constant that states what happens now. Closing
//! the gap means flipping the constant, which turns on the principle's own
//! assertion. Where a boundary was decided instead (a direct reply restarts a
//! declared wait's reminder clock), the test pins exactly what may change.
use super::*;
use std::collections::{BTreeMap, BTreeSet};
use tokio::io::AsyncWriteExt;

const MASTER: &str = "principles-master-key";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn call(
    method: reqwest::Method,
    url: String,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method, url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
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

fn admin() -> [(&'static str, &'static str); 1] {
    [("x-admin-token", MASTER)]
}

fn sess(token: &str) -> [(&'static str, &str); 1] {
    [("x-session-token", token)]
}

/// A closed Building: sessions bind identity, an invite or a group opens a
/// loca. The rate limit is off so a test can speak as much as it needs.
async fn building(db: &std::path::Path) -> (u16, ServerGuard) {
    spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
            ("RATE_LIMIT", "0".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await
}

async fn member(base: &str, name: &str, kind: &str) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/members"),
        &admin(),
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
        &[header],
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "session {name}: {status} {v}");
    v["session_token"].as_str().unwrap().to_string()
}

async fn master_session(base: &str) -> String {
    let (status, pairing) = call(
        reqwest::Method::POST,
        format!("{base}/pairings?ttl_hours=1"),
        &admin(),
        None,
    )
    .await;
    assert!(status < 300, "pairing: {status} {pairing}");
    session(
        base,
        ("x-pairing-code", pairing["pairing_code"].as_str().unwrap()),
        "",
        "user",
    )
    .await
}

async fn create_room(base: &str, room: &str, extensions: Option<&[&str]>) {
    let mut body = serde_json::json!({ "name": room });
    if let Some(list) = extensions {
        body["extensions"] = serde_json::json!(list);
    }
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        &admin(),
        Some(body),
    )
    .await;
    assert_eq!(status, 201, "create {room}: {v}");
}

async fn seat(base: &str, room: &str, name: &str) {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/invites"),
        &admin(),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert!(status < 300, "seat {name} in {room}: {status} {v}");
}

async fn principal_of(base: &str, name: &str) -> String {
    let (_, profiles) = call(
        reqwest::Method::GET,
        format!("{base}/profiles"),
        &admin(),
        None,
    )
    .await;
    profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == name)
        .unwrap_or_else(|| panic!("no profile named {name}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A group `group` founded by Smaster `founder` with `members`, and one group
/// loca `room` in it. Returns the founder's session.
async fn group_loca(
    base: &str,
    founder: &str,
    group: &str,
    room: &str,
    members: &[&str],
) -> String {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &admin(),
        Some(serde_json::json!({ "name": founder })),
    )
    .await;
    assert!(status < 300, "smaster {founder}: {status} {v}");
    let key = v["token"].as_str().unwrap().to_string();
    let lead = session(base, ("x-admin-token", &key), founder, "user").await;
    let (status, g) = call(
        reqwest::Method::POST,
        format!("{base}/groups"),
        &sess(&lead),
        Some(serde_json::json!({ "name": group })),
    )
    .await;
    assert_eq!(status, 201, "{g}");
    let gid = g["id"].as_str().unwrap().to_string();
    for name in members {
        let id = principal_of(base, name).await;
        let (status, v) = call(
            reqwest::Method::PUT,
            format!("{base}/groups/{gid}/members/{id}"),
            &sess(&lead),
            None,
        )
        .await;
        assert_eq!(status, 200, "add {name} to {group}: {v}");
    }
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        &sess(&lead),
        Some(serde_json::json!({ "name": room, "group": gid })),
    )
    .await;
    assert_eq!(status, 201, "group loca {room}: {v}");
    lead
}

async fn ws_as(port: u16, room: &str, name: &str, kind: &str, session: &str) -> Ws {
    let mut ws = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={room}&name={name}&type={kind}"),
        &["loca.v1".into(), format!("loca.session.{session}")],
    )
    .await;
    // The history frame comes after the socket subscribed to the room, so
    // every later broadcast reaches it.
    wait_for(&mut ws, |v| v["t"] == "history").await;
    ws
}

async fn ws_send(ws: &mut Ws, frame: Value) {
    ws.send(WsMessage::Text(frame.to_string().into()))
        .await
        .unwrap();
}

/// Every text frame up to and including the first that matches `until`.
async fn frames_until<F: Fn(&Value) -> bool>(ws: &mut Ws, until: F) -> Vec<Value> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let msg = ws.next().await.expect("stream ended").expect("ws error");
            if let WsMessage::Text(txt) = msg {
                let v: Value = serde_json::from_str(&txt).unwrap();
                let done = until(&v);
                seen.push(v);
                if done {
                    return;
                }
            }
        }
    })
    .await
    .expect("timed out waiting for frame");
    seen
}

fn is_msg(v: &Value, text: &str) -> bool {
    v["t"] == "msg" && v["message"]["text"] == text
}

/// Every message in `room` after `since`, read the way an absent reader
/// backfills: `?since=` pages of `page` until one comes back empty.
async fn backfill(
    base: &str,
    room: &str,
    header: (&str, &str),
    since: u64,
    page: usize,
) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let mut cursor = since;
    loop {
        let (status, v) = call(
            reqwest::Method::GET,
            format!("{base}/rooms/{room}/messages?since={cursor}&limit={page}"),
            &[header],
            None,
        )
        .await;
        assert_eq!(status, 200, "backfill: {v}");
        let batch = v.as_array().unwrap().clone();
        let Some(last) = batch.last() else {
            return out;
        };
        cursor = last["id"].as_u64().unwrap();
        out.extend(batch);
    }
}

// ============================================================================
// 2.2 — "Conversation is conversation": speech creates no work record.
// ============================================================================

/// Work records: what exists only after an explicit act (PRINCIPLES "Four
/// principles" 3, "A task", "A goal", "Silence is not intent; waiting is
/// explicit state", "Loca extensions"). Speech must neither create nor change
/// a row in any of these.
const WORK_RECORDS: &[&str] = &[
    "tasks",
    // A task's review history: written only by the review moves.
    "task_reviews",
    "goals",
    "waits",
    "notes",
    "note_revisions",
    "journal",
    // The archived loca memory and the Loca Wiki (pages, history, and the
    // editor's review jobs with their leases and receipts).
    "loca_memory",
    "loca_memory_entries",
    "loca_wiki",
    "loca_wiki_pages",
    "loca_wiki_history",
    "loca_wiki_batches",
    "loca_wiki_leases",
    "loca_wiki_receipts",
    // The Vault's rules: written only by an explicit act of those who run
    // the loca (PRINCIPLES "Vault").
    "loca_vault",
];

/// Everything else in the schema. None of these is a work record: the words
/// themselves and their parts, delivery and care bookkeeping (wire details by
/// PRINCIPLES "Wire concepts such as Attention, Care ..."), idempotency and
/// audit, and Building or loca state that a post may legitimately touch
/// (a round-robin turn lives in `rooms`). A post may write these.
const NOT_WORK_RECORDS: &[&str] = &[
    // The message, its reactions and its attachments.
    "messages",
    "message_reactions",
    "attachment_blobs",
    "attachment_pending",
    "attachment_refs",
    // Delivery and care: durable outbox, attention ledger, cooldown marks.
    "attentions",
    "care_outbox",
    "care_marks",
    // Wiki context delivery to an agent runtime (receipts, the pages it
    // asked to receive, its readiness): delivery, not the Wiki itself.
    "wiki_context_selection",
    "wiki_context_receipts",
    "wiki_native_runtime",
    // Loca state: mode, settings (round-robin turn), seats and door.
    "rooms",
    "invites",
    "bans",
    "birebirs",
    "room_operator_assignments",
    // Building identity, credentials and sessions.
    "members",
    "member_suspensions",
    "principals",
    "credentials",
    "principal_sessions",
    "principal_preferences",
    "smasters",
    "admin_sessions",
    "pairing_log",
    "admission_stock",
    "join_requests",
    // Keys claimed with an invite (`POST /membership/claim` and its ACK, and
    // the delivery-copy expiry): Building credentials, written by no post.
    "membership_claims",
    // Groups and Building notifications.
    "groups",
    "group_members",
    "group_rooms",
    "group_log",
    "notifications",
    "notification_receipts",
    // Storage housekeeping.
    "storage_maintenance",
];

/// A decided boundary, not a gap (docs/adr/0003-recorded-decisions.md, entry
/// 9): a post whose structured `target` names someone who is waiting for the
/// sender re-wakes that waiter (`Hub::post`, "P0#1") and, in the same
/// transaction, restarts the reminder clock of that declared wait: `since`
/// moves to the reply, `last_signal_at` and `signal_count` reset. That is the
/// whole of it. The wait itself (who waits, for whom, why) is not created,
/// completed, cleared or reworded by speech, and no other work record
/// changes. These are the columns of `waits` a reply may touch.
const WAIT_CLOCK_COLUMNS: [usize; 3] = [4, 5, 6]; // since, last_signal_at, signal_count
const WAIT_COLUMNS: usize = 7;

type Snapshot = BTreeMap<String, Vec<String>>;

/// Every user table of the database, and each one's rows in a stable order.
fn snapshot(db: &std::path::Path) -> Snapshot {
    let conn = rusqlite::Connection::open(db).unwrap();
    let names: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut out = Snapshot::new();
    for name in names {
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{name}\"")).unwrap();
        let columns = stmt.column_count();
        let mut rows: Vec<String> = stmt
            .query_map([], |row| {
                (0..columns)
                    .map(|i| row.get_ref(i).map(cell))
                    .collect::<Result<Vec<_>, _>>()
                    .map(|cells| cells.join(" | "))
            })
            .unwrap()
            .map(Result::unwrap)
            .collect();
        rows.sort();
        out.insert(name, rows);
    }
    out
}

fn cell(value: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match value {
        ValueRef::Null => "NULL".into(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(r) => r.to_string(),
        ValueRef::Text(t) => format!("{:?}", String::from_utf8_lossy(t)),
        ValueRef::Blob(b) => format!("blob:{b:?}"),
    }
}

/// The tables whose rows differ between two snapshots.
fn changed(before: &Snapshot, after: &Snapshot) -> BTreeSet<String> {
    before
        .keys()
        .chain(after.keys())
        .filter(|table| before.get(*table) != after.get(*table))
        .cloned()
        .collect()
}

fn work_changes(
    before: &Snapshot,
    after: &Snapshot,
) -> BTreeMap<String, (Vec<String>, Vec<String>)> {
    changed(before, after)
        .into_iter()
        .filter(|table| WORK_RECORDS.contains(&table.as_str()))
        .map(|table| {
            let was = before.get(&table).cloned().unwrap_or_default();
            let now = after.get(&table).cloned().unwrap_or_default();
            (table, (was, now))
        })
        .collect()
}

/// Every table in the schema is classified: a new table fails here until
/// someone decides whether speech may write it.
fn assert_every_table_classified(snapshot: &Snapshot) {
    let unclassified: Vec<&String> = snapshot
        .keys()
        .filter(|table| {
            !WORK_RECORDS.contains(&table.as_str()) && !NOT_WORK_RECORDS.contains(&table.as_str())
        })
        .collect();
    assert!(
        unclassified.is_empty(),
        "unclassified tables {unclassified:?}: add each to WORK_RECORDS (speech must not \
         write it) or NOT_WORK_RECORDS (a post may write it)"
    );
    for table in WORK_RECORDS.iter().chain(NOT_WORK_RECORDS) {
        assert!(
            snapshot.contains_key(*table),
            "{table} is classified but no longer in the schema"
        );
    }
}

fn message_rows(snapshot: &Snapshot) -> usize {
    snapshot["messages"].len()
}

#[tokio::test]
async fn speech_creates_no_work_record_and_explicit_acts_do() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("speech.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    // Cast: the Master, a human and an agent at an ordinary table, the same
    // two in a group loca and in a direct message the human opened.
    let master = master_session(&base).await;
    let ayse_key = member(&base, "ayse", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    create_room(&base, "talk", None).await;
    seat(&base, "talk", "ayse").await;
    seat(&base, "talk", "bot").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;
    group_loca(&base, "kaan", "crew", "crew-room", &["ayse", "bot"]).await;
    let (status, opened) = call(
        Method::POST,
        format!("{base}/birebirs"),
        &sess(&ayse),
        Some(serde_json::json!({ "name": "bot" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let dm = opened["birebir"]["room"].as_str().unwrap().to_string();

    // Work records that already exist, so a change to one is visible: a
    // declared task, a declared wait (bot waits for ayse), a note, a Journal
    // line. No goal and no lead yet. Tasks live on the Loca Board, which
    // needs a Lead: it is on while the task is declared, then off again and
    // the Lead cleared (the task is kept).
    super::loca_board::turn_board_on(&base, MASTER, "talk", "ayse").await;
    for (path, who, body) in [
        (
            "tasks",
            &master,
            serde_json::json!({"title": "declared task", "assigned_to": "bot", "by": "master"}),
        ),
        (
            "waits",
            &bot,
            serde_json::json!({"waiting_for": "ayse", "reason": "needs ayse's review", "by": "bot"}),
        ),
        (
            "notes",
            &ayse,
            serde_json::json!({"key": "plan", "title": "Plan", "body": "v1", "by": "ayse"}),
        ),
        (
            "journal",
            &bot,
            serde_json::json!({"text": "shipped the parser", "by": "bot"}),
        ),
    ] {
        let (status, v) = call(
            Method::POST,
            format!("{base}/rooms/talk/{path}"),
            &sess(who),
            Some(body),
        )
        .await;
        assert_eq!(status, 201, "explicit {path}: {v}");
    }
    let (status, v) = call(
        Method::DELETE,
        format!("{base}/rooms/talk/extensions/board?open_tasks=keep"),
        &admin(),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/lead"),
        &admin(),
        Some(serde_json::json!({"lead": null})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    // One upload per loca, cited later by a message there.
    let mut uploads = BTreeMap::new();
    for (room, who) in [("talk", &ayse), ("crew-room", &bot), (dm.as_str(), &ayse)] {
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend_from_slice(room.as_bytes());
        let r = reqwest::Client::new()
            .post(format!("{base}/rooms/{room}/attachments"))
            .header("x-session-token", who.as_str())
            .header("content-type", "image/png")
            .header("x-filename", "shot.png")
            .body(png)
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "upload to {room}: {}", r.status());
        let v: Value = r.json().await.unwrap();
        uploads.insert(room.to_string(), v["id"].as_str().unwrap().to_string());
    }
    // Sockets for the WebSocket half, opened before the snapshot so that
    // taking a seat is not counted as speech.
    let mut sockets = Vec::new();
    for room in ["talk", "crew-room", dm.as_str()] {
        for (name, kind, token) in [("ayse", "user", &ayse), ("bot", "agent", &bot)] {
            let ws = ws_as(port, room, name, kind, token).await;
            sockets.push((room.to_string(), name, ws));
        }
    }

    let before = snapshot(&db);
    assert_every_table_classified(&before);

    // ---- Speech over REST: a human and an agent, in every kind of loca, with
    // a mention, a reply, an attachment, and words that look like work or like
    // a command. None of it is an explicit act.
    let mut said = 0;
    let mut last_in: BTreeMap<String, u64> = BTreeMap::new();
    for room in ["talk", "crew-room", dm.as_str()] {
        let mut last_id: Option<u64> = None;
        for (name, token, peer) in [("ayse", &ayse, "bot"), ("bot", &bot, "ayse")] {
            let plain = format!("{name}: TODO fix the login bug, task: deploy, I'll wait for you");
            // ayse addressing bot with a structured target in `talk` is the
            // known gap below, tested on its own; here she mentions in text.
            let target = (room != "talk" || name != "ayse").then_some(peer);
            let mut bodies = vec![
                serde_json::json!({"text": plain}),
                serde_json::json!({
                    "text": format!("@{peer} could you look at this?"),
                    "target": target,
                }),
                serde_json::json!({"text": "@goal ship v2"}),
                serde_json::json!({"text": format!("@lead {peer}")}),
                serde_json::json!({"text": "please @loca-care check on this"}),
            ];
            if let Some(id) = last_id {
                bodies.push(serde_json::json!({"text": "agreed, done", "reply_to": id}));
            }
            if (room == "crew-room") == (name == "bot") {
                bodies.push(serde_json::json!({
                    "text": "screenshot attached",
                    "attachments": [uploads[room].clone()],
                }));
            }
            for mut body in bodies {
                body["sender"] = serde_json::json!(name);
                body["sender_type"] =
                    serde_json::json!(if name == "bot" { "agent" } else { "user" });
                body["op_id"] = serde_json::json!(format!("speech-{said}"));
                let (status, v) = call(
                    Method::POST,
                    format!("{base}/rooms/{room}/messages"),
                    &sess(token),
                    Some(body.clone()),
                )
                .await;
                assert_eq!(status, 201, "{room} {name} {body}: {v}");
                last_id = v["id"].as_u64();
                last_in.insert(room.to_string(), last_id.unwrap());
                said += 1;
            }
        }
    }
    // The operator's own `@goal` and `@lead` typed as a message, sent the way
    // any client that does not intercept the composer command would.
    for text in ["@goal ship v2", "@lead bot"] {
        let (status, v) = call(
            Method::POST,
            format!("{base}/rooms/talk/messages"),
            &sess(&master),
            Some(serde_json::json!({"sender": "Master", "sender_type": "user", "text": text})),
        )
        .await;
        assert_eq!(status, 201, "{text}: {v}");
        said += 1;
    }

    // ---- Speech over the WebSocket, the same shapes.
    for (room, name, ws) in sockets.iter_mut() {
        let peer = if *name == "ayse" { "bot" } else { "ayse" };
        let skip_target = room == "talk" && *name == "ayse";
        let reply_to = last_in[room.as_str()];
        for (i, frame) in [
            serde_json::json!({"t": "send", "text": format!("ws {name}: TODO, task: review")}),
            serde_json::json!({
                "t": "send",
                "text": format!("ws @{peer} please take this"),
                "target": (!skip_target).then_some(peer),
            }),
            serde_json::json!({"t": "send", "text": format!("ws {name}: @goal ship v3")}),
            serde_json::json!({"t": "send", "text": format!("ws {name}: replying"), "reply_to": reply_to}),
        ]
        .into_iter()
        .enumerate()
        {
            let text = frame["text"].as_str().unwrap().to_string();
            let mut frame = frame;
            frame["op_id"] = serde_json::json!(format!("ws-{room}-{name}-{i}"));
            ws_send(ws, frame).await;
            wait_for(ws, |v| is_msg(v, &text)).await;
            said += 1;
        }
    }

    let after = snapshot(&db);
    assert_every_table_classified(&after);
    assert_eq!(
        message_rows(&after),
        message_rows(&before) + said,
        "every word above was accepted and stored"
    );
    let work = work_changes(&before, &after);
    assert!(
        work.is_empty(),
        "speech changed work records (PRINCIPLES \"Conversation is conversation\"): {work:#?}"
    );
    let (_, settings) = call(
        Method::GET,
        format!("{base}/rooms/talk/settings"),
        &sess(&master),
        None,
    )
    .await;
    assert_eq!(
        settings["lead"],
        Value::Null,
        "`@lead` in a message names no lead"
    );

    // ---- Naming the waiter only in the text is ordinary speech: the wait
    // is not touched. Only the structured address below reaches it.
    let before_mention = snapshot(&db);
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/messages"),
        &sess(&ayse),
        Some(serde_json::json!({
            "sender": "ayse", "sender_type": "user",
            "text": "@bot I have not forgotten your review",
        })),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let work = work_changes(&before_mention, &snapshot(&db));
    assert!(
        work.is_empty(),
        "a free-text mention of the waiter changed work records: {work:#?}"
    );

    // ---- A direct, structured reply to the person who is waiting for you.
    let before_reply = snapshot(&db);
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/messages"),
        &sess(&ayse),
        Some(serde_json::json!({
            "sender": "ayse", "sender_type": "user", "target": "bot",
            "text": "@bot here is the review you were waiting for",
        })),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let after_reply = snapshot(&db);
    let work = work_changes(&before_reply, &after_reply);
    assert_eq!(
        work.keys().collect::<Vec<_>>(),
        ["waits"],
        "a direct reply to a waiter touches that wait's clock and no other work record: {work:#?}"
    );
    let (was, now) = &work["waits"];
    assert_eq!((was.len(), now.len()), (1, 1), "no wait created or cleared");
    let cells = |row: &str| row.split(" | ").map(str::to_string).collect::<Vec<_>>();
    let (old, new) = (cells(&was[0]), cells(&now[0]));
    assert_eq!(
        (old.len(), new.len()),
        (WAIT_COLUMNS, WAIT_COLUMNS),
        "the waits table changed shape; decide what a reply may touch in the new column"
    );
    for column in 0..WAIT_COLUMNS {
        if WAIT_CLOCK_COLUMNS.contains(&column) {
            continue;
        }
        assert_eq!(
            old[column], new[column],
            "a reply changed column {column} of the wait (room, waiter, waited-for, reason are not its to change)"
        );
    }
    assert_ne!(old[4], new[4], "the wait's clock restarts at the reply");
    assert_eq!(new[6], "0", "the signal count starts again: {new:?}");

    // ---- The other side of the boundary: the explicit acts do create.
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/lead"),
        &sess(&master),
        Some(serde_json::json!({"lead": "bot"})),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["lead"], "bot", "POST /lead names the lead");
    let (status, v) = call(
        Method::PUT,
        format!("{base}/rooms/talk/extensions/board"),
        &admin(),
        None,
    )
    .await;
    assert_eq!(status, 200, "{v}");
    let before_goal = snapshot(&db);
    assert!(
        before_goal["goals"].is_empty(),
        "no goal from any word above"
    );
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/goals"),
        &sess(&master),
        Some(serde_json::json!({"outcome": "ship v2", "by": "master"})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let (status, v) = call(
        Method::POST,
        format!("{base}/rooms/talk/tasks"),
        &sess(&master),
        Some(serde_json::json!({"title": "write the release notes", "by": "master"})),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let after_goal = snapshot(&db);
    let created: Vec<String> = work_changes(&before_goal, &after_goal)
        .into_keys()
        .collect();
    assert_eq!(
        created,
        ["goals", "tasks"],
        "the explicit acts wrote their records"
    );
    assert_eq!(after_goal["goals"].len(), 1);
    assert!(after_goal["goals"][0].contains("ship v2"));
    assert_eq!(after_goal["tasks"].len(), before_goal["tasks"].len() + 1);
}

// ============================================================================
// 2.3 — "A word spoken to you must not disappear."
// ============================================================================
//
// PRINCIPLES.md / PRINCIPLES.tr.md (TR), the reliability paragraph after the
// spirit test: "A word spoken to you must not disappear. Messages are not
// lost, a message received twice does not produce the same side effect
// twice, a restart does not kill the room ..." Each test below names the
// sentence it pins. (Identity impersonation is pinned elsewhere.)

/// "Messages are not lost" and "a restart does not kill the room": every
/// message the server answered 201 for is there after the process is killed
/// (SIGKILL, no shutdown path) and started again on the same database; a
/// reader that was away when they were said gets each exactly once and in
/// order, from the archive (`?since=`) and from its reconnect history; the
/// room keeps counting after the restart instead of reusing ids. "A message
/// received twice does not produce the same side effect twice": the same
/// operation re-sent after the restart returns the original.
#[tokio::test]
async fn an_acknowledged_word_survives_a_restart_and_reaches_the_absent_reader() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("kept.db");
    let (port, guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ayse_key = member(&base, "ayse", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    create_room(&base, "kept", None).await;
    seat(&base, "kept", "ayse").await;
    seat(&base, "kept", "bot").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;

    // The reader hears the first word live, then goes away.
    let mut reader = ws_as(port, "kept", "bot", "agent", &bot).await;
    let post = |text: String, op: String| {
        let base = base.clone();
        let ayse = ayse.clone();
        async move {
            call(
                Method::POST,
                format!("{base}/rooms/kept/messages"),
                &sess(&ayse),
                Some(serde_json::json!({
                    "sender": "ayse", "sender_type": "user", "text": text, "op_id": op,
                })),
            )
            .await
        }
    };
    let (status, first) = post("before you left".into(), "kept-first".into()).await;
    assert_eq!(status, 201);
    let seen = wait_for(&mut reader, |v| is_msg(v, "before you left")).await;
    let last_seen = seen["message"]["id"].as_u64().unwrap();
    assert_eq!(first["id"].as_u64(), Some(last_seen));
    drop(reader);

    // More than one history tail (200) is said while the reader is away.
    let mut acked: Vec<(u64, String)> = Vec::new();
    for i in 0..210 {
        let text = format!("while you were away {i}");
        let (status, v) = post(text.clone(), format!("away-{i}")).await;
        assert_eq!(status, 201, "{v}");
        acked.push((v["id"].as_u64().unwrap(), text));
    }
    assert!(acked.windows(2).all(|w| w[0].0 < w[1].0));

    // Killed, not shut down: nothing gets a chance to flush on the way out.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;

    // The reader's backfill: each acknowledged word once, in order.
    let missed: Vec<(u64, String)> =
        backfill(&base, "kept", ("x-session-token", &bot), last_seen, 100)
            .await
            .iter()
            .map(|m| {
                (
                    m["id"].as_u64().unwrap(),
                    m["text"].as_str().unwrap().to_string(),
                )
            })
            .collect();
    assert_eq!(
        missed, acked,
        "the archive gives every missed word once, in order"
    );

    // Its reconnect history: the newest tail, in order, no repeats.
    let mut reader = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room=kept&name=bot&type=agent"),
        &["loca.v1".into(), format!("loca.session.{bot}")],
    )
    .await;
    let history = wait_for(&mut reader, |v| v["t"] == "history").await;
    let tail: Vec<(u64, String)> = history["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["id"].as_u64().unwrap(),
                m["text"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(!tail.is_empty());
    assert!(
        acked.ends_with(&tail),
        "history after a restart is the newest acknowledged words, in order and once"
    );

    // The same operation re-sent after the restart is the same message.
    let (last_id, last_text) = acked.last().unwrap().clone();
    let (status, again) = call(
        Method::POST,
        format!("{base}/rooms/kept/messages"),
        &sess(&ayse),
        Some(serde_json::json!({
            "sender": "ayse", "sender_type": "user", "text": last_text, "op_id": "away-209",
        })),
    )
    .await;
    assert!(status < 300, "{again}");
    assert_eq!(again["id"].as_u64(), Some(last_id));
    // A new word after the restart is new and comes after every old one.
    let (status, fresh) = call(
        Method::POST,
        format!("{base}/rooms/kept/messages"),
        &sess(&ayse),
        Some(serde_json::json!({
            "sender": "ayse", "sender_type": "user", "text": "after the restart", "op_id": "kept-after",
        })),
    )
    .await;
    assert_eq!(status, 201, "{fresh}");
    let fresh_id = fresh["id"].as_u64().unwrap();
    assert!(fresh_id > last_id, "ids are not reused after a restart");
    let live = wait_for(&mut reader, |v| v["t"] == "msg").await;
    assert_eq!(
        live["message"]["id"].as_u64(),
        Some(fresh_id),
        "the re-sent operation was not broadcast again"
    );
    let all = backfill(&base, "kept", ("x-session-token", &bot), 0, 1000).await;
    assert_eq!(all.len(), 1 + acked.len() + 1, "no duplicate rows");
}

/// "A message received twice does not produce the same side effect twice":
/// a client whose connection dropped before it read the answer cannot know
/// whether the word landed, so it sends the same operation again. Exactly one
/// message results, the re-send answers with its id, and the room hears it
/// once. Over REST (answer lost after the server accepted; connection dropped
/// right after the request was written) and over the WebSocket, and the same
/// operation re-sent over the other transport or under a renewed session.
#[tokio::test]
async fn a_resend_after_a_dropped_connection_lands_exactly_once() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("resend.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ayse_key = member(&base, "ayse", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    create_room(&base, "again", None).await;
    seat(&base, "again", "ayse").await;
    seat(&base, "again", "bot").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;
    let mut listener = ws_as(port, "again", "bot", "agent", &bot).await;

    let body = |text: &str, op: &str| serde_json::json!({"sender": "ayse", "sender_type": "user", "text": text, "op_id": op});
    let raw_request = |body: &Value| {
        let body = body.to_string();
        format!(
            "POST /rooms/again/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\
             content-type: application/json\r\nx-session-token: {ayse}\r\n\
             content-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        )
    };

    // 1. The server accepted the word, the answer was never read.
    let first = body("answer lost", "drop-after-accept");
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(raw_request(&first).as_bytes()).await.unwrap();
    let heard = wait_for(&mut listener, |v| is_msg(v, "answer lost")).await;
    drop(tcp);
    let (status, again) = call(
        Method::POST,
        format!("{base}/rooms/again/messages"),
        &sess(&ayse),
        Some(first),
    )
    .await;
    assert!(status < 300, "{again}");
    assert_eq!(
        again["id"], heard["message"]["id"],
        "the re-send answers with the original"
    );

    // 2. The connection dropped right after the request was written: the word
    // may or may not have landed; the re-send makes it exactly one.
    let second = body("dropped at once", "drop-at-once");
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(raw_request(&second).as_bytes())
        .await
        .unwrap();
    drop(tcp);
    let (status, resent) = call(
        Method::POST,
        format!("{base}/rooms/again/messages"),
        &sess(&ayse),
        Some(second),
    )
    .await;
    assert!(status < 300, "{resent}");

    // 3. Over the WebSocket: the socket is gone before the echo came back; a
    // new socket sends the same operation, then a marker.
    let mut speaker = ws_as(port, "again", "ayse", "user", &ayse).await;
    ws_send(
        &mut speaker,
        serde_json::json!({"t": "send", "text": "socket lost", "op_id": "ws-drop"}),
    )
    .await;
    drop(speaker);
    let mut speaker = ws_as(port, "again", "ayse", "user", &ayse).await;
    ws_send(
        &mut speaker,
        serde_json::json!({"t": "send", "text": "socket lost", "op_id": "ws-drop"}),
    )
    .await;
    ws_send(
        &mut speaker,
        serde_json::json!({"t": "send", "text": "marker", "op_id": "ws-marker"}),
    )
    .await;
    wait_for(&mut speaker, |v| is_msg(v, "marker")).await;
    // The same operation by the same person over REST, and over a socket
    // under a new session of hers.
    let (status, over_rest) = call(
        Method::POST,
        format!("{base}/rooms/again/messages"),
        &sess(&ayse),
        Some(body("socket lost", "ws-drop")),
    )
    .await;
    assert!(status < 300, "{over_rest}");
    let renewed = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let mut speaker = ws_as(port, "again", "ayse", "user", &renewed).await;
    ws_send(
        &mut speaker,
        serde_json::json!({"t": "send", "text": "socket lost", "op_id": "ws-drop"}),
    )
    .await;
    ws_send(
        &mut speaker,
        serde_json::json!({"t": "send", "text": "marker 2", "op_id": "ws-marker-2"}),
    )
    .await;
    wait_for(&mut speaker, |v| is_msg(v, "marker 2")).await;

    // The room heard each word once; the archive holds each once.
    let heard = frames_until(&mut listener, |v| is_msg(v, "marker")).await;
    let count = |text: &str| heard.iter().filter(|v| is_msg(v, text)).count();
    assert_eq!(count("dropped at once"), 1, "{heard:#?}");
    assert_eq!(count("socket lost"), 1, "{heard:#?}");
    let all = backfill(&base, "again", ("x-session-token", &bot), 0, 1000).await;
    let stored = |text: &str| -> Vec<u64> {
        all.iter()
            .filter(|m| m["text"] == text)
            .map(|m| m["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(stored("answer lost").len(), 1);
    assert_eq!(stored("dropped at once"), [resent["id"].as_u64().unwrap()]);
    // One operation, one message, whichever transport and session re-sent it.
    assert_eq!(
        stored("socket lost"),
        [over_rest["id"].as_u64().unwrap()],
        "{all:#?}"
    );
    assert_eq!(all.len(), 5, "{all:#?}");
}

/// "A message received twice does not produce the same side effect twice",
/// whichever way it is received twice. Each sender says a word once over the
/// WebSocket; the same operation re-sent over REST, under a new session, and
/// over either transport after a restart answers with that word and adds
/// nothing. The Master (its sessions), a member (its own-key session, then an
/// invite session) and a Smaster. Two senders that use the same `op_id`, even
/// under the same display name, are two operations, and neither is answered
/// with the other's word.
#[tokio::test]
async fn one_operation_is_one_message_across_transports_sessions_and_a_restart() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("one-op.db");
    let (port, guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ayse_key = member(&base, "ayse", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    create_room(&base, "one", None).await;
    let (status, invite) = call(
        Method::POST,
        format!("{base}/rooms/one/invites"),
        &admin(),
        Some(serde_json::json!({ "name": "ayse" })),
    )
    .await;
    assert!(status < 300, "{invite}");
    let ayse_invite = invite["token"].as_str().unwrap().to_string();
    seat(&base, "one", "bot").await;
    // A Smaster wearing the member's display name. No door lets one take it
    // now, so the rows are written the way an earlier version stored them.
    let (status, v) = call(
        Method::POST,
        format!("{base}/smasters"),
        &admin(),
        Some(serde_json::json!({ "name": "ayse" })),
    )
    .await;
    assert_eq!(status, 409, "the door refuses a member's name: {v}");
    let (status, v) = call(
        Method::POST,
        format!("{base}/smasters"),
        &admin(),
        Some(serde_json::json!({ "name": "old-deputy" })),
    )
    .await;
    assert!(status < 300, "{v}");
    let deputy_code = v["token"].as_str().unwrap().to_string();
    drop(guard);
    wait_for_server_shutdown(port).await;
    let connection = rusqlite::Connection::open(&db).unwrap();
    for rename in [
        "UPDATE smasters SET name = 'ayse' WHERE name = 'old-deputy'",
        "UPDATE principals SET display_name = 'ayse' WHERE display_name = 'old-deputy'",
    ] {
        assert_eq!(connection.execute(rename, []).unwrap(), 1, "{rename}");
    }
    drop(connection);
    let (port, guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");

    let post = |base: String, token: String, text: &'static str, op: &'static str| async move {
        call(
            Method::POST,
            format!("{base}/rooms/one/messages"),
            &sess(&token),
            Some(serde_json::json!({
                "sender": "ignored", "sender_type": "user", "text": text, "op_id": op,
            })),
        )
        .await
    };
    let say_over_socket = |token: String, text: &'static str, op: &'static str| async move {
        let mut ws = ws_as(port, "one", "ignored", "user", &token).await;
        ws_send(
            &mut ws,
            serde_json::json!({"t": "send", "text": text, "op_id": op}),
        )
        .await;
        let echo = wait_for(&mut ws, |v| is_msg(v, text)).await;
        echo["message"]["id"].as_u64().unwrap()
    };

    // Each sender's word, said once over the socket.
    let master = master_session(&base).await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let deputy = session(&base, ("x-admin-token", &deputy_code), "ayse", "user").await;
    let words = [
        ("the master's word", "op-master"),
        ("ayse's word", "op-ayse"),
        ("the deputy's word", "op-deputy"),
    ];
    let mut ids = Vec::new();
    for (token, (text, op)) in [&master, &ayse, &deputy].into_iter().zip(words) {
        ids.push(say_over_socket(token.clone(), text, op).await);
    }

    // The same operations over REST, under the same and under new sessions.
    let renewed = [
        master_session(&base).await,
        session(&base, ("x-room-token", &ayse_invite), "ayse", "user").await,
        session(&base, ("x-admin-token", &deputy_code), "ayse", "user").await,
    ];
    for (i, (text, op)) in words.into_iter().enumerate() {
        for token in [[&master, &ayse, &deputy][i], &renewed[i]] {
            let (status, again) = post(base.clone(), token.clone(), text, op).await;
            assert!(status < 300, "{text}: {again}");
            assert_eq!(again["id"].as_u64(), Some(ids[i]), "{text} over REST");
        }
    }

    // A restart, new sessions, both transports again.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let after = [
        master_session(&base).await,
        session(&base, ("x-room-token", &ayse_invite), "ayse", "user").await,
        session(&base, ("x-admin-token", &deputy_code), "ayse", "user").await,
    ];
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;
    for (i, (text, op)) in words.into_iter().enumerate() {
        let (status, again) = post(base.clone(), after[i].clone(), text, op).await;
        assert!(status < 300, "{text}: {again}");
        assert_eq!(
            again["id"].as_u64(),
            Some(ids[i]),
            "{text} over REST after the restart"
        );
        let mut ws = ws_as(port, "one", "ignored", "user", &after[i]).await;
        ws_send(
            &mut ws,
            serde_json::json!({"t": "send", "text": text, "op_id": op}),
        )
        .await;
        let marker = format!("marker {i}");
        ws_send(
            &mut ws,
            serde_json::json!({"t": "send", "text": marker, "op_id": format!("marker-{i}")}),
        )
        .await;
        wait_for(&mut ws, |v| {
            v["t"] == "msg" && v["message"]["text"] == marker
        })
        .await;
    }

    // Two senders, one `op_id` string, the same display name: two words.
    let (status, from_ayse) = post(base.clone(), after[1].clone(), "ayse says", "same-op").await;
    assert_eq!(status, 201, "{from_ayse}");
    let (status, from_deputy) =
        post(base.clone(), after[2].clone(), "the deputy says", "same-op").await;
    assert_eq!(status, 201, "{from_deputy}");
    assert_eq!(from_deputy["text"], "the deputy says", "{from_deputy}");
    assert_ne!(from_ayse["id"], from_deputy["id"]);

    let all = backfill(&base, "one", ("x-session-token", &bot), 0, 1000).await;
    let count = |text: &str| all.iter().filter(|m| m["text"] == text).count();
    for (text, _) in words {
        assert_eq!(count(text), 1, "{text}: {all:#?}");
    }
    assert_eq!(all.len(), words.len() + 3 + 2, "{all:#?}");
}

// ============================================================================
// 2.4 — "Loca extensions › Boundaries": an extension that is off is refused.
// ============================================================================
//
// PRINCIPLES "Loca extensions", Boundaries: "An extension that is off is not
// merely hidden; the server refuses it. Removing one does not delete its
// records." The principle names no status. Today's refusal is 403 with a
// body naming the extension (`routes::extensions::extension_off`); these
// tests record that explicitly. "A birebir has no extensions" is pinned in
// `extensions.rs`.

/// The refusal text of the extension fence.
fn fenced(status: u16, v: &Value, id: &str) -> bool {
    status == 403
        && v.as_str().is_some_and(|text| {
            text.contains(&format!("this extension is off in this loca: {id} "))
        })
}

/// Every route of each extension in the catalog, as (extension, method,
/// path template as registered in `main.rs`, body). `live` has no route of
/// its own; its one door is turning live mode on through the settings, which
/// the fence does not cover. `every_fenced_route_is_in_the_matrix` holds
/// this list to the router.
fn route_matrix() -> Vec<(&'static str, reqwest::Method, &'static str, Option<Value>)> {
    use reqwest::Method;
    let json = |v: Value| Some(v);
    let any = || json(serde_json::json!({}));
    vec![
        ("notes", Method::GET, "/rooms/:id/notes", None),
        (
            "notes",
            Method::POST,
            "/rooms/:id/notes",
            json(serde_json::json!({"key": "probe", "title": "t", "body": "b", "by": "x"})),
        ),
        ("notes", Method::GET, "/rooms/:id/notes/:key", None),
        (
            "notes",
            Method::PUT,
            "/rooms/:id/notes/:key",
            json(serde_json::json!({"body": "changed", "by": "x"})),
        ),
        ("notes", Method::DELETE, "/rooms/:id/notes/:key", None),
        ("notes", Method::GET, "/rooms/:id/notes/:key/history", None),
        ("board", Method::GET, "/rooms/:id/tasks", None),
        ("board", Method::GET, "/rooms/:id/tasks/:tid", None),
        (
            "board",
            Method::POST,
            "/rooms/:id/tasks",
            json(serde_json::json!({"title": "probe", "by": "x"})),
        ),
        (
            "board",
            Method::PATCH,
            "/rooms/:id/tasks/:tid",
            json(serde_json::json!({"status": "done", "by": "x"})),
        ),
        ("focus", Method::GET, "/rooms/:id/goals", None),
        (
            "focus",
            Method::POST,
            "/rooms/:id/goals",
            json(serde_json::json!({"outcome": "probe", "by": "x"})),
        ),
        (
            "focus",
            Method::PATCH,
            "/rooms/:id/goals/:gid",
            json(serde_json::json!({"outcome": "probe", "by": "x"})),
        ),
        ("journal", Method::GET, "/rooms/:id/journal", None),
        (
            "journal",
            Method::POST,
            "/rooms/:id/journal",
            json(serde_json::json!({"text": "probe"})),
        ),
        ("wiki", Method::GET, "/rooms/:id/wiki", None),
        ("wiki", Method::GET, "/rooms/:id/wiki/pages/:slug", None),
        (
            "wiki",
            Method::PUT,
            "/rooms/:id/wiki/pages/:slug",
            json(
                serde_json::json!({"title": "Home", "body": "probe", "sources": [], "reason": "probe"}),
            ),
        ),
        ("wiki", Method::DELETE, "/rooms/:id/wiki/pages/:slug", None),
        ("wiki", Method::PUT, "/rooms/:id/wiki/config", any()),
        ("wiki", Method::POST, "/rooms/:id/wiki/review", any()),
        ("wiki", Method::GET, "/rooms/:id/wiki/maintenance", None),
        (
            "wiki",
            Method::GET,
            "/rooms/:id/wiki/context/requests",
            None,
        ),
        ("wiki", Method::POST, "/rooms/:id/wiki/runtime", any()),
        ("wiki", Method::GET, "/rooms/:id/wiki/context", None),
        ("wiki", Method::POST, "/rooms/:id/wiki/context", any()),
        (
            "wiki",
            Method::POST,
            "/rooms/:id/wiki/context/accept",
            any(),
        ),
        ("wiki", Method::POST, "/rooms/:id/wiki/context/offer", any()),
        (
            "wiki",
            Method::POST,
            "/rooms/:id/wiki/maintenance/claim",
            any(),
        ),
        (
            "wiki",
            Method::POST,
            "/rooms/:id/wiki/maintenance/finish",
            any(),
        ),
        (
            "wiki",
            Method::POST,
            "/rooms/:id/wiki/maintenance/renew",
            any(),
        ),
        ("vault", Method::GET, "/rooms/:id/vault", None),
        ("vault", Method::GET, "/rooms/:id/vault/history", None),
        (
            "vault",
            Method::PUT,
            "/rooms/:id/vault",
            json(serde_json::json!({"body": "rules", "base_rev": 0})),
        ),
        (
            "live",
            Method::PUT,
            "/rooms/:id/settings",
            json(serde_json::json!({"live": true})),
        ),
    ]
}

/// `route_matrix` with its templates filled in for `room`, as (extension,
/// method, path, body).
fn routes_of(room: &str) -> Vec<(&'static str, reqwest::Method, String, Option<Value>)> {
    route_matrix()
        .into_iter()
        .map(|(ext, method, template, body)| {
            let path = template
                .split('/')
                .map(|part| match part {
                    ":id" => room,
                    ":key" => "plan",
                    ":slug" => "home",
                    ":tid" | ":gid" => "1",
                    other => other,
                })
                .collect::<Vec<_>>()
                .join("/");
            (ext, method, path, body)
        })
        .collect()
}

/// The (method, path) pairs registered on the main router in `main.rs`, read
/// from its source: every `.route("path", <method>(...)...)` between
/// `let app = Router::new()` and the `extension_fence` layer that wraps them.
/// The router cannot be enumerated from outside the binary, so its source is
/// the smallest honest place to read it from.
fn registered_routes(main_rs: &str) -> BTreeSet<(String, String)> {
    // Only literal `.route(` calls before the first layer are read below; a
    // `.merge`/`.nest`/fallback or a second router anywhere in the served
    // chain, or a route after the first layer, must fail rather than add
    // paths this list never sees.
    super::authority_matrix::assert_plain_composition(
        main_rs,
        "let app = Router::new()",
        ".expect(\"serve\")",
        ".route_layer(",
    );
    let start = main_rs
        .find("let app = Router::new()")
        .expect("main.rs builds `app` with Router::new()");
    let end = start
        + main_rs[start..]
            .find("extension_fence")
            .expect("the extension fence wraps the main router");
    let mut rest = &main_rs[start..end];
    let mut out = BTreeSet::new();
    while let Some(at) = rest.find(".route(") {
        let args = &rest[at + ".route(".len()..];
        let mut depth = 1;
        let close = args
            .char_indices()
            .find(|&(_, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                depth == 0
            })
            .map(|(i, _)| i)
            .expect("balanced .route(...)");
        let call = &args[..close];
        let open = call.find('"').expect("route path literal");
        let len = call[open + 1..].find('"').unwrap();
        let path = &call[open + 1..open + 1 + len];
        let chain = &call[open + 1 + len + 1..];
        for method in ["get", "post", "put", "patch", "delete"] {
            for (pos, _) in chain.match_indices(&format!("{method}(")) {
                let before = chain[..pos].chars().last();
                if before.is_none_or(|c| !(c.is_alphanumeric() || c == '_')) {
                    out.insert((
                        method.to_uppercase(),
                        super::authority_matrix::colon_path(path),
                    ));
                }
            }
        }
        rest = &args[close..];
    }
    out
}

/// The extension that owns each `/rooms/:id/<segment>`, as the registry
/// (`protocol::EXTENSIONS`) claims it; the extension fence reads the same
/// table. A segment claimed twice is a mistake in the registry.
fn fence_segments() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for spec in protocol::EXTENSIONS {
        for segment in spec.routes {
            if let Some(other) = out.insert(segment.to_string(), spec.id.to_string()) {
                panic!(
                    "/rooms/:id/{segment} is claimed by both {other} and {}",
                    spec.id
                );
            }
        }
    }
    assert!(!out.is_empty(), "no extension claims a route");
    out
}

/// The `/rooms/:id/<segment>` routes that are the loca's core: always on,
/// never behind the extension fence ("Chat is always on"; attentions, waits,
/// reminders and memory are not extensions). `""` is `/rooms/:id` itself.
const CORE_LOCA_SEGMENTS: &[&str] = &[
    "",
    // The Master's archive and delete of a closed loca: its lifecycle, as a
    // seal (`""`) is, whatever extensions it has.
    "archive",
    "attachments",
    "attentions",
    "call",
    "care",
    "extensions",
    "group",
    "invites",
    "lead",
    "members",
    "memory",
    "messages",
    "mode",
    "moderate",
    "operators",
    "reactions",
    "records",
    "release",
    "search",
    "settings",
    // An agent runtime's ephemeral "replying" signal: chat, not an extension.
    "typing",
    "waits",
];

/// The first segment after `/rooms/:id`, or `None` for a route outside a loca.
fn loca_segment(path: &str) -> Option<&str> {
    if path == "/rooms/:id" {
        return Some("");
    }
    path.strip_prefix("/rooms/:id/")
        .map(|tail| tail.split('/').next().unwrap_or(""))
}

/// Every route of a loca is either claimed by an extension in the registry
/// or listed as core: a new `/rooms/:id/...` route cannot reach the router
/// without someone deciding whether the extension fence covers it. A claim
/// or a core entry the router does not serve is stale and fails too, as does
/// a segment that is both.
#[test]
fn every_loca_route_is_claimed_by_an_extension_or_core() {
    let claimed = fence_segments();
    let registered = registered_routes(include_str!("../../src/main.rs"));
    let served: BTreeSet<&str> = registered
        .iter()
        .filter_map(|(_, path)| loca_segment(path))
        .collect();
    let mut problems = Vec::new();
    for (method, path) in &registered {
        let Some(segment) = loca_segment(path) else {
            continue;
        };
        if !claimed.contains_key(segment) && !CORE_LOCA_SEGMENTS.contains(&segment) {
            problems.push(format!(
                "{method} {path}: /rooms/:id/{segment} is neither claimed by an extension \
                 (`routes` in protocol::EXTENSIONS) nor listed in CORE_LOCA_SEGMENTS"
            ));
        }
    }
    for (segment, ext) in &claimed {
        if CORE_LOCA_SEGMENTS.contains(&segment.as_str()) {
            problems.push(format!("/rooms/:id/{segment} is both core and {ext}'s"));
        }
        if !served.contains(segment.as_str()) {
            problems.push(format!(
                "{ext} claims /rooms/:id/{segment}, which the router does not serve"
            ));
        }
    }
    for segment in CORE_LOCA_SEGMENTS {
        if !served.contains(segment) {
            problems.push(format!(
                "CORE_LOCA_SEGMENTS lists /rooms/:id/{segment}, which the router does not serve"
            ));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}

/// Coverage is enforced: every route the router registers under a segment the
/// extension fence maps must be in `route_matrix` with the extension the fence
/// gives it, and the matrix holds nothing the router does not register.
#[test]
fn every_fenced_route_is_in_the_matrix() {
    let segments = fence_segments();
    let registered: BTreeSet<(String, String)> =
        registered_routes(include_str!("../../src/main.rs"))
            .into_iter()
            .filter(|(_, path)| {
                path.strip_prefix("/rooms/:id/")
                    .and_then(|tail| tail.split('/').next())
                    .is_some_and(|segment| segments.contains_key(segment))
            })
            .collect();
    let mut matrix = BTreeSet::new();
    for (ext, method, template, _) in route_matrix() {
        if ext == "live" {
            continue;
        }
        let segment = template
            .strip_prefix("/rooms/:id/")
            .and_then(|tail| tail.split('/').next())
            .unwrap();
        assert_eq!(
            segments.get(segment).map(String::as_str),
            Some(ext),
            "{method} {template} is listed under {ext}, the fence says otherwise"
        );
        matrix.insert((method.to_string(), template.to_string()));
    }
    let missing: Vec<_> = registered.difference(&matrix).collect();
    let stale: Vec<_> = matrix.difference(&registered).collect();
    assert!(
        missing.is_empty(),
        "routes under the extension fence with no entry in route_matrix: {missing:?}"
    );
    assert!(
        stale.is_empty(),
        "route_matrix entries the router does not register: {stale:?}"
    );
}

async fn name_lead(base: &str, room: &str, lead: &str) {
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/lead"),
        &admin(),
        Some(serde_json::json!({ "lead": lead })),
    )
    .await;
    assert_eq!(status, 200, "name {room}'s Lead: {v}");
}

async fn set_extensions(base: &str, room: &str, enabled: &[&str]) {
    let (status, v) = call(
        reqwest::Method::PUT,
        // Unfinished tasks, if any, are kept (the old behaviour).
        format!("{base}/rooms/{room}/extensions?open_tasks=keep"),
        &admin(),
        Some(serde_json::json!({ "enabled": enabled })),
    )
    .await;
    assert_eq!(status, 200, "set {room} extensions: {v}");
}

/// The catalog as the server serves it. A new extension without routes in
/// `route_matrix` fails here, so it cannot slip past these tests.
async fn catalog(base: &str) -> Vec<String> {
    let (status, v) = call(
        reqwest::Method::GET,
        format!("{base}/extensions"),
        &admin(),
        None,
    )
    .await;
    assert_eq!(status, 200);
    let ids: Vec<String> = v["catalog"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spec| spec["id"].as_str().unwrap().to_string())
        .collect();
    let probed: BTreeSet<&str> = routes_of("x").iter().map(|(ext, ..)| *ext).collect();
    for id in &ids {
        assert!(
            probed.contains(id.as_str()),
            "extension {id} is in the catalog but has no routes in route_matrix"
        );
    }
    ids
}

/// A member of the loca is refused on every route of an extension that is
/// off, reads and writes alike, with 403 and the extension's name; with it on
/// the same request is not fenced. Someone who may not enter the loca gets
/// exactly the same answer whether the extension is on or off — a member of
/// another loca, a stranger with no credential, and, for a group loca, a
/// Smaster and a member outside the group.
#[tokio::test]
async fn an_extension_that_is_off_is_refused_and_outsiders_learn_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("fence.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ids = catalog(&base).await;
    let all: Vec<&str> = ids.iter().map(String::as_str).collect();

    let ayse_key = member(&base, "ayse", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    let veli_key = member(&base, "veli", "user").await;
    create_room(&base, "fenced", None).await;
    create_room(&base, "elsewhere", None).await;
    seat(&base, "fenced", "ayse").await;
    seat(&base, "fenced", "bot").await;
    seat(&base, "elsewhere", "veli").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;
    let veli = session(&base, ("x-room-token", &veli_key), "veli", "user").await;
    group_loca(&base, "kaan", "crew", "crew-room", &["ayse"]).await;
    let (status, v) = call(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        &admin(),
        Some(serde_json::json!({ "name": "other" })),
    )
    .await;
    assert!(status < 300, "{v}");
    let other = session(
        &base,
        ("x-admin-token", v["token"].as_str().unwrap()),
        "other",
        "user",
    )
    .await;
    // A note to read and change, in both locas.
    for room in ["fenced", "crew-room"] {
        let (status, v) = call(
            reqwest::Method::POST,
            format!("{base}/rooms/{room}/notes"),
            &sess(&ayse),
            Some(serde_json::json!({"key": "plan", "title": "Plan", "body": "v1", "by": "ayse"})),
        )
        .await;
        assert_eq!(status, 201, "{v}");
    }

    // Members, extension off: every route refused with the fence's 403. The
    // operator side of `live` is the Master's (participants cannot change
    // settings at all), so the Master stands in for that one door.
    set_extensions(&base, "fenced", &[]).await;
    for (who, token) in [("ayse", &ayse), ("bot", &bot)] {
        for (ext, method, path, body) in routes_of("fenced") {
            let headers: Vec<(&str, &str)> = if ext == "live" {
                admin().to_vec()
            } else {
                sess(token).to_vec()
            };
            let (status, v) = call(method.clone(), format!("{base}{path}"), &headers, body).await;
            assert!(
                fenced(status, &v, ext),
                "{who}: {method} {path} with {ext} off must be refused: {status} {v}"
            );
        }
    }
    // With every extension on, none of those requests meets the fence. The
    // Loca Board needs a Lead to be turned on.
    name_lead(&base, "fenced", "bot").await;
    name_lead(&base, "crew-room", "ayse").await;
    set_extensions(&base, "fenced", &all).await;
    for (ext, method, path, body) in routes_of("fenced") {
        if method != reqwest::Method::GET && ext != "live" {
            continue; // reads only: the writes above must not land later
        }
        let headers: Vec<(&str, &str)> = if ext == "live" {
            admin().to_vec()
        } else {
            sess(&ayse).to_vec()
        };
        let (status, v) = call(method.clone(), format!("{base}{path}"), &headers, body).await;
        assert!(
            !fenced(status, &v, ext),
            "{method} {path} with {ext} on: {status} {v}"
        );
    }
    let (status, _) = call(
        reqwest::Method::PUT,
        format!("{base}/rooms/fenced/settings"),
        &admin(),
        Some(serde_json::json!({"live": false})),
    )
    .await;
    assert_eq!(status, 200);

    // Outsiders: the same answer, status and body, with everything on and
    // with everything off.
    type Outsider<'a> = (&'a str, &'a str, Vec<(&'a str, &'a str)>);
    let outsiders: Vec<Outsider> = vec![
        ("fenced", "member of another loca", sess(&veli).to_vec()),
        ("fenced", "no credential", Vec::new()),
        (
            "crew-room",
            "Smaster outside the group",
            sess(&other).to_vec(),
        ),
        (
            "crew-room",
            "member outside the group",
            sess(&veli).to_vec(),
        ),
        ("crew-room", "agent outside the group", sess(&bot).to_vec()),
    ];
    let mut answers: BTreeMap<String, (u16, Value)> = BTreeMap::new();
    for phase in ["on", "off"] {
        let enabled: &[&str] = if phase == "on" { &all } else { &[] };
        set_extensions(&base, "fenced", enabled).await;
        set_extensions(&base, "crew-room", enabled).await;
        for (room, who, headers) in &outsiders {
            let mut doors = routes_of(room);
            for extra in ["extensions", "settings"] {
                doors.push((
                    "-",
                    reqwest::Method::GET,
                    format!("/rooms/{room}/{extra}"),
                    None,
                ));
            }
            for (_, method, path, body) in doors {
                let answer = call(method.clone(), format!("{base}{path}"), headers, body).await;
                assert!(
                    answer.0 >= 400,
                    "{who} reached {method} {path} in {room}: {answer:?}"
                );
                let key = format!("{who} {method} {path}");
                match answers.get(&key) {
                    None => {
                        answers.insert(key, answer);
                    }
                    Some(on) => assert_eq!(
                        &answer, on,
                        "{key}: the answer with extensions off differs from on"
                    ),
                }
            }
        }
    }
}

/// Turning an extension off while it is in use closes it at once: the same
/// open client's next request is refused, and no frame of a switched-off
/// extension reaches its open socket (the client sends no extension frames:
/// `ClientFrame` is send, control and typing). What was refused meanwhile does
/// not come back when the extension is turned on again, and what was there
/// before is still there.
#[tokio::test]
async fn turning_an_extension_off_closes_it_at_once_and_back_on_resurrects_nothing() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("in-use.db");
    let (port, _guard) = building(&db).await;
    let base = format!("http://127.0.0.1:{port}");
    let ids = catalog(&base).await;
    let all: Vec<&str> = ids.iter().map(String::as_str).collect();
    let master = master_session(&base).await;
    let ayse_key = member(&base, "ayse", "user").await;
    member(&base, "bot", "agent").await;
    create_room(&base, "busy", None).await;
    seat(&base, "busy", "ayse").await;
    seat(&base, "busy", "bot").await;
    let ayse = session(&base, ("x-room-token", &ayse_key), "ayse", "user").await;
    // The Loca Board, in the catalog, needs a Lead to be turned on; the task
    // below is its record.
    name_lead(&base, "busy", "bot").await;
    set_extensions(&base, "busy", &all).await;
    let mut tab = ws_as(port, "busy", "ayse", "user", &ayse).await;
    let post = |path: &'static str, token: String, body: Value| {
        let base = base.clone();
        async move {
            call(
                Method::POST,
                format!("{base}/rooms/busy/{path}"),
                &sess(&token),
                Some(body),
            )
            .await
        }
    };

    // In use: a note, a task and a Journal line, each heard on the open tab.
    let kept: [(&str, String, Value, &str); 3] = [
        (
            "notes",
            ayse.clone(),
            serde_json::json!({"key": "kept", "title": "Kept", "body": "before", "by": "ayse"}),
            "note",
        ),
        (
            "tasks",
            master.clone(),
            serde_json::json!({"title": "kept task", "by": "master"}),
            "task",
        ),
        (
            "journal",
            ayse.clone(),
            serde_json::json!({"text": "kept line"}),
            "journal",
        ),
    ];
    for (path, token, body, frame) in kept {
        let (status, v) = post(path, token, body).await;
        assert_eq!(status, 201, "{path}: {v}");
        wait_for(&mut tab, |v| v["t"] == frame).await;
    }
    let wiki_before = call(
        Method::GET,
        format!("{base}/rooms/busy/wiki"),
        &sess(&ayse),
        None,
    )
    .await;

    // Off, all four at once (live stays): the very next request is refused.
    set_extensions(&base, "busy", &["live"]).await;
    for (ext, method, path, body) in routes_of("busy") {
        if ext == "live" {
            continue;
        }
        let (status, v) = call(method.clone(), format!("{base}{path}"), &sess(&ayse), body).await;
        assert!(fenced(status, &v, ext), "{method} {path}: {status} {v}");
    }
    // The operator is refused too, on what would create records.
    for (path, body, ext) in [
        (
            "tasks",
            serde_json::json!({"title": "refused task", "by": "master"}),
            "board",
        ),
        (
            "goals",
            serde_json::json!({"outcome": "refused goal", "by": "master"}),
            "focus",
        ),
        (
            "notes",
            serde_json::json!({"key": "refused", "title": "R", "body": "no", "by": "master"}),
            "notes",
        ),
        (
            "journal",
            serde_json::json!({"text": "refused line"}),
            "journal",
        ),
    ] {
        let (status, v) = post(path, master.clone(), body).await;
        assert!(fenced(status, &v, ext), "master {path}: {status} {v}");
    }
    let (status, _) = post(
        "messages",
        ayse.clone(),
        serde_json::json!({
            "sender": "ayse", "sender_type": "user", "text": "marker off",
        }),
    )
    .await;
    assert_eq!(status, 201);
    let while_off = frames_until(&mut tab, |v| is_msg(v, "marker off")).await;
    // Every frame the registry says an extension sends.
    let owned: BTreeSet<&str> = protocol::EXTENSIONS
        .iter()
        .flat_map(|spec| spec.frames.iter().copied())
        .collect();
    assert!(
        owned.contains("note") && owned.contains("journal"),
        "{owned:?}"
    );
    let extension_frames = |frames: &[Value]| -> Vec<Value> {
        frames
            .iter()
            .filter(|v| owned.contains(v["t"].as_str().unwrap_or("")))
            .cloned()
            .collect()
    };
    let leaked = extension_frames(&while_off);
    // The line that records the change is written (and read once the
    // Journal is on again, below), but it is not pushed to a socket while
    // the Journal is off: off means refused, not hidden.
    assert!(
        leaked.is_empty(),
        "frames of switched-off extensions: {leaked:#?}"
    );
    assert!(
        while_off
            .iter()
            .any(|v| v["t"] == "settings"
                && v["settings"]["extensions"] == serde_json::json!(["live"])),
        "the open tab is told the set changed"
    );

    // Back on: only what was there before; nothing refused comes back, and
    // no frame announces a refused record.
    set_extensions(&base, "busy", &all).await;
    let (status, _) = post(
        "messages",
        ayse.clone(),
        serde_json::json!({
            "sender": "ayse", "sender_type": "user", "text": "marker on",
        }),
    )
    .await;
    assert_eq!(status, 201);
    let back_on = frames_until(&mut tab, |v| is_msg(v, "marker on")).await;
    let announced = extension_frames(&back_on);
    assert!(
        announced.iter().all(|v| v["t"] == "journal"
            && v["entry"]["text"]
                .as_str()
                .is_some_and(|t| t.starts_with("extensions: added"))),
        "turning on announces only the change: {announced:#?}"
    );
    let get = |path: &'static str| {
        let base = base.clone();
        let ayse = ayse.clone();
        async move {
            let (status, v) = call(
                Method::GET,
                format!("{base}/rooms/busy/{path}"),
                &sess(&ayse),
                None,
            )
            .await;
            assert_eq!(status, 200, "{path}: {v}");
            v
        }
    };
    let notes = get("notes").await;
    let keys: Vec<&str> = notes
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["kept"], "{notes}");
    let tasks = get("tasks").await;
    let titles: Vec<&str> = tasks
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["kept task"]);
    assert_eq!(get("goals").await, serde_json::json!([]));
    let journal = get("journal").await.to_string();
    assert!(journal.contains("kept line"));
    assert!(!journal.contains("refused"), "{journal}");
    assert_eq!(
        call(
            Method::GET,
            format!("{base}/rooms/busy/wiki"),
            &sess(&ayse),
            None
        )
        .await,
        wiki_before,
        "the Wiki answers as it did before"
    );
}
