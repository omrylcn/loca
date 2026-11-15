//! A loca's identity (PRINCIPLES "A loca's identity"): its description, its
//! own number of seats, an icon and colour from closed lists, and labels.
//! Set at creation or later through `PUT /rooms/:id/settings` by whoever may
//! change the loca's extensions, read by whoever may enter, journaled, kept
//! across a restart, a rename and a close, refused in a direct message. The seats
//! hold at both doors: the invite mint and the socket.
use super::*;

const MASTER: &str = "ident-master-key";

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

const ADMIN: (&str, &str) = ("x-admin-token", MASTER);

async fn create(base: &str, body: Value) -> (u16, Value) {
    send(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        ADMIN,
        Some(body),
    )
    .await
}

async fn settings(base: &str, room: &str, header: (&str, &str)) -> (u16, Value) {
    send(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/settings"),
        header,
        None,
    )
    .await
}

async fn put_settings(base: &str, room: &str, header: (&str, &str), body: Value) -> (u16, Value) {
    send(
        reqwest::Method::PUT,
        format!("{base}/rooms/{room}/settings"),
        header,
        Some(body),
    )
    .await
}

async fn member(base: &str, name: &str, kind: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/members"),
        ADMIN,
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "admit {name}: {status} {v}");
    v["token"].as_str().unwrap().to_string()
}

async fn invite(base: &str, room: &str, name: &str) -> (u16, Value) {
    send(
        reqwest::Method::POST,
        format!("{base}/rooms/{room}/invites"),
        ADMIN,
        Some(serde_json::json!({ "name": name })),
    )
    .await
}

async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let (status, v) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        header,
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "session {name}: {status} {v}");
    v["session_token"].as_str().unwrap().to_string()
}

async fn principal_of(base: &str, name: &str) -> String {
    let (_, profiles) = send(
        reqwest::Method::GET,
        format!("{base}/profiles"),
        ADMIN,
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

async fn journal(base: &str, room: &str) -> Vec<String> {
    let (status, v) = send(
        reqwest::Method::GET,
        format!("{base}/rooms/{room}/journal"),
        ADMIN,
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

fn identity_of(v: &Value) -> Value {
    serde_json::json!({
        "description": v["description"],
        "seats": v["seats"],
        "icon": v["icon"],
        "color": v["color"],
        "labels": v["labels"],
    })
}

async fn closed_building(db: &std::path::Path, extra: &[(&str, String)]) -> (u16, ServerGuard) {
    let mut env = vec![
        ("DB_PATH", db.to_str().unwrap().to_string()),
        ("REQUIRE_SESSIONS", "1".to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    env.extend(extra.iter().map(|(k, v)| (*k, v.clone())));
    spawn_server_env(MASTER, &env).await
}

/// Set at creation, read by whoever may enter, edited by the loca's
/// managers only, journaled, refused field by field, kept across a restart,
/// a rename and a close, and absent from a direct message.
#[tokio::test]
async fn a_locas_identity_is_set_read_governed_journaled_and_kept() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("identity.db");
    let (port, guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    use reqwest::Method;

    // An older client sends a name only: the loca has no identity at all,
    // and its settings answer carries none of the new keys.
    assert_eq!(
        create(&base, serde_json::json!({"name": "plain"})).await.0,
        201
    );
    let (_, plain) = settings(&base, "plain", ADMIN).await;
    for key in ["description", "seats", "icon", "color", "labels"] {
        assert!(plain.get(key).is_none(), "{key} in {plain}");
    }

    // Every refused class at creation, and nothing is created.
    let refused = [
        serde_json::json!({"description": "two\nlines"}),
        serde_json::json!({"description": "hidden\u{200B}space"}),
        serde_json::json!({"description": "x".repeat(281)}),
        serde_json::json!({"seats": 1}),
        serde_json::json!({"seats": 33}),
        serde_json::json!({"icon": "https://example.invalid/x.svg"}),
        serde_json::json!({"icon": "star"}),
        serde_json::json!({"color": "#ff0000"}),
        serde_json::json!({"labels": ["a", "b", "c", "d", "e", "f"]}),
        serde_json::json!({"labels": ["x".repeat(25)]}),
        serde_json::json!({"labels": ["Master"]}),
        serde_json::json!({"labels": ["wiki-editor"]}),
        serde_json::json!({"labels": [" "]}),
    ];
    for (n, fields) in refused.iter().enumerate() {
        let mut body = fields.clone();
        body["name"] = serde_json::json!(format!("bad-{n}"));
        let (status, v) = create(&base, body).await;
        assert_eq!(status, 400, "{fields}: {v}");
        let (status, _) = settings(&base, &format!("bad-{n}"), ADMIN).await;
        assert_ne!(status, 200, "a refused create leaves no loca ({fields})");
    }

    // Created with everything; labels are trimmed and deduplicated.
    let (status, v) = create(
        &base,
        serde_json::json!({
            "name": "proj",
            "extensions": ["notes", "journal"],
            "description": "  Ship release 14 without losing a word.  ",
            "seats": 5,
            "icon": "rocket",
            "color": "sky",
            "labels": ["backend", " q4 ", "Backend"],
        }),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let (_, s) = settings(&base, "proj", ADMIN).await;
    let created = serde_json::json!({
        "description": "Ship release 14 without losing a word.",
        "seats": 5, "icon": "rocket", "color": "sky", "labels": ["backend", "q4"],
    });
    assert_eq!(identity_of(&s), created);
    assert_eq!(
        journal(&base, "proj").await,
        [
            "Master | extensions: opened with Notes, Journal",
            "Master | identity: opened with description; 5 seats; icon rocket; colour sky; labels backend, q4",
        ]
    );

    // The cast: an appointed operator, a participant, an agent, a Smaster.
    let oper_key = member(&base, "oper", "user").await;
    let part_key = member(&base, "part", "user").await;
    let bot_key = member(&base, "bot", "agent").await;
    let lobby_key = member(&base, "lobbyist", "user").await;
    for name in ["oper", "part", "bot"] {
        assert!(invite(&base, "proj", name).await.0 < 300);
    }
    let oper = session(&base, ("x-room-token", &oper_key), "oper", "user").await;
    let part = session(&base, ("x-room-token", &part_key), "part", "user").await;
    let bot = session(&base, ("x-room-token", &bot_key), "bot", "agent").await;
    let lobby = session(&base, ("x-room-token", &lobby_key), "lobbyist", "user").await;
    let oper_id = principal_of(&base, "oper").await;
    send(
        Method::DELETE,
        format!("{base}/rooms/proj/operators"),
        ADMIN,
        None,
    )
    .await;
    let (status, v) = send(
        Method::POST,
        format!("{base}/rooms/proj/operators"),
        ADMIN,
        Some(serde_json::json!({ "principal_id": oper_id })),
    )
    .await;
    assert!(status < 300, "appoint: {v}");
    let (_, deputy_key) = send(
        Method::POST,
        format!("{base}/smasters"),
        ADMIN,
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    let deputy = session(
        &base,
        ("x-admin-token", deputy_key["token"].as_str().unwrap()),
        "deputy",
        "user",
    )
    .await;

    // Everyone at the table reads it, in the settings and in the loca list;
    // someone who may not enter learns nothing.
    for who in [&oper, &part, &bot, &deputy] {
        let (status, s) = settings(&base, "proj", ("x-session-token", who)).await;
        assert_eq!(status, 200);
        assert_eq!(identity_of(&s), created);
        let (_, rooms) = send(
            Method::GET,
            format!("{base}/rooms"),
            ("x-session-token", who),
            None,
        )
        .await;
        let row = rooms
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["room"] == "proj")
            .unwrap_or_else(|| panic!("proj not listed: {rooms}"));
        assert_eq!(row["description"], created["description"]);
        assert_eq!(row["icon"], "rocket");
        assert_eq!(row["color"], "sky");
        assert_eq!(row["labels"], created["labels"]);
        assert!(
            row.get("seats").is_none(),
            "the list carries no seat number"
        );
    }
    let (status, _) = settings(&base, "proj", ("x-session-token", &lobby)).await;
    assert!(matches!(status, 401 | 403 | 404));
    let (_, rooms) = send(
        Method::GET,
        format!("{base}/rooms"),
        ("x-session-token", &lobby),
        None,
    )
    .await;
    assert!(!rooms.to_string().contains("Ship release 14"), "{rooms}");

    // Only the operator, the Master and Smasters change it.
    for who in [&part, &bot, &lobby] {
        let (status, v) = put_settings(
            &base,
            "proj",
            ("x-session-token", who),
            serde_json::json!({"description": "taken over"}),
        )
        .await;
        assert!(matches!(status, 401 | 403 | 404), "{status} {v}");
    }
    let (status, v) = put_settings(
        &base,
        "proj",
        ("x-session-token", &oper),
        serde_json::json!({"description": "Ship release 14.", "seats": 6}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["description"], "Ship release 14.");
    assert_eq!(v["seats"], 6);
    let (status, v) = put_settings(
        &base,
        "proj",
        ("x-session-token", &deputy),
        serde_json::json!({"icon": "", "labels": ["release"]}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert!(v.get("icon").is_none(), "an empty icon clears it: {v}");
    let (status, v) = put_settings(
        &base,
        "proj",
        ADMIN,
        serde_json::json!({"color": "purple", "icon": "flask"}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    // A refused edit keeps everything, and says why.
    let (status, v) = put_settings(
        &base,
        "proj",
        ("x-session-token", &oper),
        serde_json::json!({"labels": ["Operator"]}),
    )
    .await;
    assert_eq!(status, 400);
    assert!(v.as_str().unwrap().contains("grants nothing"), "{v}");
    // Identity is sent on its own.
    let (status, v) = put_settings(
        &base,
        "proj",
        ("x-session-token", &oper),
        serde_json::json!({"description": "x", "rate_limit": 5}),
    )
    .await;
    assert_eq!(status, 400, "{v}");
    // Saying the same again changes nothing and writes no line.
    let (status, _) =
        put_settings(&base, "proj", ADMIN, serde_json::json!({"color": "purple"})).await;
    assert_eq!(status, 200);
    let changed = serde_json::json!({
        "description": "Ship release 14.", "seats": 6, "icon": "flask",
        "color": "purple", "labels": ["release"],
    });
    assert_eq!(
        identity_of(&settings(&base, "proj", ADMIN).await.1),
        changed
    );
    let lines = journal(&base, "proj").await;
    assert_eq!(
        lines[2..],
        [
            "oper | identity: description changed; seats 5 → 6",
            "deputy | identity: icon removed; labels release",
            "Master | identity: icon flask; colour purple",
        ]
    );

    // A direct message has none: its settings carry none and nobody sets them.
    let (status, opened) = send(
        Method::POST,
        format!("{base}/birebirs"),
        ("x-session-token", &oper),
        Some(serde_json::json!({ "name": "part" })),
    )
    .await;
    assert_eq!(status, 201, "{opened}");
    let bb = opened["birebir"]["room"].as_str().unwrap().to_string();
    for who in [&oper, &part] {
        let (status, v) = put_settings(
            &base,
            &bb,
            ("x-session-token", who),
            serde_json::json!({"description": "our corner"}),
        )
        .await;
        assert!(matches!(status, 401 | 403), "{status} {v}");
    }
    let (_, s) = settings(&base, &bb, ("x-session-token", &oper)).await;
    assert!(s.get("description").is_none(), "{s}");
    assert_eq!(
        create(
            &base,
            serde_json::json!({"name": "bb-forged", "description": "x"})
        )
        .await
        .0,
        409,
        "the direct message id space is not a loca's"
    );

    // Closed: the identity stays and does not change.
    let (status, _) =
        put_settings(&base, "proj", ADMIN, serde_json::json!({"archived": true})).await;
    assert_eq!(status, 200);
    assert_eq!(
        identity_of(&settings(&base, "proj", ADMIN).await.1),
        changed
    );
    let (status, v) = put_settings(
        &base,
        "proj",
        ADMIN,
        serde_json::json!({"description": "after closing"}),
    )
    .await;
    assert_eq!(status, 409, "{v}");
    assert!(v.as_str().unwrap().contains("closed"), "{v}");
    let (status, _) =
        put_settings(&base, "proj", ADMIN, serde_json::json!({"archived": false})).await;
    assert_eq!(status, 200);

    // A restart keeps it.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        identity_of(&settings(&base, "proj", ADMIN).await.1),
        changed
    );

    // A rename carries it.
    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) =
        closed_building(&db, &[("ROOM_RENAME", "proj:release-14".to_string())]).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(
        identity_of(&settings(&base, "release-14", ADMIN).await.1),
        changed
    );
    assert_eq!(
        journal(&base, "release-14").await.len(),
        5,
        "the journal lines moved with it"
    );
}

/// An older database: a loca stored before identity existed reads with no
/// identity and the Building's seats, and takes one without losing anything.
#[tokio::test]
async fn a_loca_stored_before_identity_existed_is_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("older.db");
    {
        let (port, _guard) = closed_building(&db, &[]).await;
        let base = format!("http://127.0.0.1:{port}");
        assert_eq!(
            create(&base, serde_json::json!({"name": "old"})).await.0,
            201
        );
        let (status, _) =
            put_settings(&base, "old", ADMIN, serde_json::json!({"rate_limit": 3})).await;
        assert_eq!(status, 200);
    }
    // Write the settings exactly as an older server stored them: no
    // identity keys at all.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let stored: String = conn
            .query_row("SELECT settings FROM rooms WHERE room = 'old'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let mut value: Value = serde_json::from_str(&stored).unwrap();
        for key in ["description", "seats", "icon", "color", "labels"] {
            assert!(
                value.get(key).is_none(),
                "{key} stored for a loca without it"
            );
            value.as_object_mut().unwrap().remove(key);
        }
        conn.execute(
            "UPDATE rooms SET settings = ?1 WHERE room = 'old'",
            [value.to_string()],
        )
        .unwrap();
    }
    let (port, _guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_, s) = settings(&base, "old", ADMIN).await;
    assert_eq!(s["rate_limit"], 3);
    assert!(s.get("seats").is_none());
    let (status, v) = put_settings(
        &base,
        "old",
        ADMIN,
        serde_json::json!({"description": "Kept since the old days."}),
    )
    .await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["rate_limit"], 3, "nothing else moved");
    assert_eq!(v["description"], "Kept since the old days.");
}

/// The seats are the loca's own: the invite mint refuses at its number, a
/// move to the default group that would mint past it is refused, the socket
/// refuses a seat past it, the number never drops below the seats held, and
/// it never rises above the Building's.
#[tokio::test]
async fn a_locas_own_seats_hold_at_every_door() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("seats.db");
    let (port, _guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    use reqwest::Method;

    // The invite mint: a loca of three seats takes three invites.
    assert_eq!(
        create(&base, serde_json::json!({"name": "small", "seats": 3}))
            .await
            .0,
        201
    );
    for n in 1..=4 {
        member(&base, &format!("s{n}"), "agent").await;
    }
    for n in 1..=3 {
        let (status, v) = invite(&base, "small", &format!("s{n}")).await;
        assert!(status < 300, "seat {n}: {status} {v}");
    }
    let (status, v) = invite(&base, "small", "s4").await;
    assert_eq!(status, 409, "the fourth invite of a three-seat loca: {v}");
    assert!(v.as_str().unwrap().contains("full"), "{v}");

    // Never below the seats held; the sentence says how many.
    let (status, v) = put_settings(&base, "small", ADMIN, serde_json::json!({"seats": 2})).await;
    assert_eq!(status, 409, "{v}");
    assert!(v.as_str().unwrap().contains("3 seats are held"), "{v}");
    assert_eq!(settings(&base, "small", ADMIN).await.1["seats"], 3);
    // Never above the Building's ceiling (LOCA_CAPACITY_MAX, 32 by default).
    let (status, v) = put_settings(&base, "small", ADMIN, serde_json::json!({"seats": 33})).await;
    assert_eq!(status, 400, "{v}");
    assert!(
        v.as_str()
            .unwrap()
            .contains("between 2 and 32 (this Building's ceiling, LOCA_CAPACITY_MAX)"),
        "{v}"
    );
    // Raised, the next invite fits.
    let (status, _) = put_settings(&base, "small", ADMIN, serde_json::json!({"seats": 4})).await;
    assert_eq!(status, 200);
    assert!(invite(&base, "small", "s4").await.0 < 300);
    assert!(journal(&base, "small")
        .await
        .contains(&"Master | identity: seats 3 → 4".to_string()));

    // A loca without its own number keeps the Building's: seven invites.
    assert_eq!(
        create(&base, serde_json::json!({"name": "wide"})).await.0,
        201
    );
    for n in 1..=8 {
        member(&base, &format!("w{n}"), "agent").await;
    }
    for n in 1..=7 {
        assert!(
            invite(&base, "wide", &format!("w{n}")).await.0 < 300,
            "w{n}"
        );
    }
    assert_eq!(invite(&base, "wide", "w8").await.0, 409);

    // A move to the default group mints an invite for each group member; a
    // loca of two seats cannot carry three, so the move is refused whole.
    let (_, group) = send(
        Method::POST,
        format!("{base}/groups"),
        ADMIN,
        Some(serde_json::json!({ "name": "crew" })),
    )
    .await;
    let group = group["id"].as_str().unwrap().to_string();
    for n in 1..=3 {
        let name = format!("g{n}");
        member(&base, &name, "user").await;
        let p = principal_of(&base, &name).await;
        let (status, v) = send(
            Method::PUT,
            format!("{base}/groups/{group}/members/{p}"),
            ADMIN,
            None,
        )
        .await;
        assert!(status < 300, "{v}");
    }
    let (status, v) = create(
        &base,
        serde_json::json!({"name": "crew-desk", "group": group, "seats": 2}),
    )
    .await;
    assert_eq!(status, 201, "{v}");
    let (_, preview) = send(
        Method::GET,
        format!("{base}/rooms/crew-desk/group/preview"),
        ADMIN,
        None,
    )
    .await;
    assert_eq!(preview["seats_needed"], 3, "{preview}");
    assert_eq!(preview["seats_free"], 2, "{preview}");
    let (status, v) = send(
        Method::PUT,
        format!("{base}/rooms/crew-desk/group"),
        ADMIN,
        Some(serde_json::json!({ "group": null })),
    )
    .await;
    assert_eq!(status, 409, "{v}");
    // Group membership holds no seat, so the number may rise to three.
    let (status, _) =
        put_settings(&base, "crew-desk", ADMIN, serde_json::json!({"seats": 3})).await;
    assert_eq!(status, 200);
    let (status, v) = send(
        Method::PUT,
        format!("{base}/rooms/crew-desk/group"),
        ADMIN,
        Some(serde_json::json!({ "group": null })),
    )
    .await;
    assert_eq!(status, 200, "{v}");
}

/// The socket door: someone who sits without an invite (here the open
/// sandbox) is refused past the loca's own number, with that number in the
/// sentence. On a default deployment that number may be above seven.
#[tokio::test]
async fn the_socket_refuses_a_seat_past_the_locas_own_number() {
    let (port, _guard) = spawn_server_env("", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let open = |name: &'static str, seats: u32| {
        let client = client.clone();
        let base = base.clone();
        async move {
            client
                .post(format!("{base}/rooms"))
                .json(&serde_json::json!({ "name": name, "seats": seats }))
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    assert_eq!(open("over", 33).await, 400, "never above the ceiling");
    assert_eq!(open("pair", 2).await, 201);
    let mut a = connect_ws(port, "pair", "ada", "agent").await;
    wait_for(&mut a, |f| f["t"] == "history").await;
    let mut b = connect_ws(port, "pair", "bo", "agent").await;
    wait_for(&mut b, |f| f["t"] == "history").await;
    let mut c = connect_ws(port, "pair", "cy", "agent").await;
    let refused = wait_for(&mut c, |f| f["t"] == "control").await;
    assert_eq!(
        refused["cmd"],
        "loca full (2 seats). ask the master for a seat, or watch with ?watch=1"
    );
    // Nine seats on a deployment that sets nothing: nine sit, the tenth is
    // refused at nine.
    assert_eq!(open("hall", 9).await, 201, "above seven by default");
    let mut seated = Vec::new();
    for n in 0..9 {
        let mut ws = connect_ws(port, "hall", &format!("h{n}"), "agent").await;
        wait_for(&mut ws, |f| f["t"] == "history").await;
        seated.push(ws);
    }
    let mut tenth = connect_ws(port, "hall", "h9", "agent").await;
    let refused = wait_for(&mut tenth, |f| f["t"] == "control").await;
    assert_eq!(
        refused["cmd"],
        "loca full (9 seats). ask the master for a seat, or watch with ?watch=1"
    );
    // The number cannot drop below the nine sitting in the hall.
    let (status, v) = send(
        reqwest::Method::PUT,
        format!("{base}/rooms/hall/settings"),
        ("x-client", "sandbox"),
        Some(serde_json::json!({"seats": 8})),
    )
    .await;
    assert_eq!(status, 409, "{v}");
    assert!(v.as_str().unwrap().contains("9 seats are held"), "{v}");
}

async fn health(base: &str) -> Value {
    send(reqwest::Method::GET, format!("{base}/health"), ADMIN, None)
        .await
        .1
}

/// A default deployment: a loca without a number has seven seats, a loca
/// may choose more up to the ceiling (32), and the invite mint holds at its
/// number. Both numbers are on `/health`.
#[tokio::test]
async fn a_default_deployment_lets_a_loca_choose_more_than_seven() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = closed_building(&dir.path().join("wide.db"), &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let h = health(&base).await;
    assert_eq!(
        (h["loca_capacity"].clone(), h["loca_capacity_max"].clone()),
        (7.into(), 32.into())
    );

    let (status, v) = create(&base, serde_json::json!({"name": "big", "seats": 12})).await;
    assert_eq!(status, 201, "{v}");
    assert_eq!(settings(&base, "big", ADMIN).await.1["seats"], 12);
    for n in 1..=13 {
        member(&base, &format!("b{n}"), "agent").await;
    }
    for n in 1..=12 {
        let (status, v) = invite(&base, "big", &format!("b{n}")).await;
        assert!(status < 300, "invite {n} of 12: {status} {v}");
    }
    assert_eq!(
        invite(&base, "big", "b13").await.0,
        409,
        "the thirteenth of twelve"
    );
    // Changed later to more, up to the ceiling.
    let (status, v) = put_settings(&base, "big", ADMIN, serde_json::json!({"seats": 32})).await;
    assert_eq!(status, 200, "{v}");
    assert_eq!(v["seats"], 32);
    assert!(invite(&base, "big", "b13").await.0 < 300);
    // A loca that chose nothing still has seven.
    assert_eq!(
        create(&base, serde_json::json!({"name": "plain"})).await.0,
        201
    );
    for n in 1..=7 {
        assert!(invite(&base, "plain", &format!("b{n}")).await.0 < 300);
    }
    assert_eq!(invite(&base, "plain", "b8").await.0, 409);
}

/// The ceiling is never below the default and never above 32.
#[tokio::test]
async fn the_ceiling_is_held_between_the_default_and_thirty_two() {
    for (capacity, max, want) in [("12", "5", 12), ("7", "40", 32), ("4", "9", 9)] {
        let (port, _guard) = spawn_server_env(
            "",
            &[
                ("LOCA_CAPACITY", capacity.to_string()),
                ("LOCA_CAPACITY_MAX", max.to_string()),
            ],
        )
        .await;
        let base = format!("http://127.0.0.1:{port}");
        let h = health(&base).await;
        assert_eq!(
            h["loca_capacity_max"], want,
            "LOCA_CAPACITY={capacity} LOCA_CAPACITY_MAX={max}"
        );
        let (status, v) =
            create(&base, serde_json::json!({"name": "top", "seats": want + 1})).await;
        assert_eq!(status, 400, "{v}");
        assert!(
            v.as_str()
                .unwrap()
                .contains(&format!("between 2 and {want}")),
            "{v}"
        );
        assert_eq!(
            create(&base, serde_json::json!({"name": "top", "seats": want}))
                .await
                .0,
            201
        );
    }
}

/// A ceiling lowered under a loca's stored number: the loca works at the
/// ceiling and its answers show the number in force; the stored number is
/// kept, and a ceiling raised again gives it back.
#[tokio::test]
async fn a_lowered_ceiling_holds_a_locas_larger_number() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("lowered.db");
    {
        let (port, guard) = closed_building(&db, &[]).await;
        let base = format!("http://127.0.0.1:{port}");
        assert_eq!(
            create(&base, serde_json::json!({"name": "wide", "seats": 15}))
                .await
                .0,
            201
        );
        for n in 1..=11 {
            member(&base, &format!("l{n}"), "agent").await;
        }
        drop(guard);
        wait_for_server_shutdown(port).await;
    }
    let stored = || {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let text: String = conn
            .query_row("SELECT settings FROM rooms WHERE room = 'wide'", [], |r| {
                r.get(0)
            })
            .unwrap();
        serde_json::from_str::<Value>(&text).unwrap()["seats"].clone()
    };
    {
        let (port, guard) = closed_building(&db, &[("LOCA_CAPACITY_MAX", "10".to_string())]).await;
        let base = format!("http://127.0.0.1:{port}");
        assert_eq!(health(&base).await["loca_capacity_max"], 10);
        assert_eq!(
            settings(&base, "wide", ADMIN).await.1["seats"],
            10,
            "the number in force"
        );
        for n in 1..=10 {
            assert!(
                invite(&base, "wide", &format!("l{n}")).await.0 < 300,
                "l{n}"
            );
        }
        assert_eq!(
            invite(&base, "wide", "l11").await.0,
            409,
            "held at the ceiling"
        );
        // Another identity edit keeps the stored number.
        let (status, v) = put_settings(
            &base,
            "wide",
            ADMIN,
            serde_json::json!({"description": "still fifteen underneath"}),
        )
        .await;
        assert_eq!(status, 200, "{v}");
        assert_eq!(v["seats"], 10);
        drop(guard);
        wait_for_server_shutdown(port).await;
    }
    assert_eq!(stored(), 15, "the stored number is kept");
    let (port, _guard) = closed_building(&db, &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    assert_eq!(settings(&base, "wide", ADMIN).await.1["seats"], 15);
    assert!(invite(&base, "wide", "l11").await.0 < 300, "fifteen again");
}

const IDENTITY_JS: &str = include_str!("../../../../web/assets/identity.js");
const STYLES_CSS: &str = include_str!("../../../../web/assets/styles.css");

/// The keys of `const <name> = { ... };` in web/assets/identity.js, in order.
fn web_keys(name: &str) -> Vec<String> {
    let start = IDENTITY_JS
        .find(&format!("const {name} = {{"))
        .unwrap_or_else(|| panic!("web/assets/identity.js declares {name}"));
    let block = &IDENTITY_JS[start..];
    let block = &block[..block.find("\n};").unwrap()];
    block
        .lines()
        .skip(1)
        .filter_map(|line| line.trim().split_once(':').map(|(key, _)| key.to_string()))
        .collect()
}

/// The browser draws exactly the icons and colours the server accepts, in
/// its order, and the stylesheet has a shade of every colour for each theme.
#[test]
fn the_web_draws_exactly_the_servers_icons_and_colours() {
    assert_eq!(web_keys("LOCA_ICON_PATHS"), protocol::LOCA_ICONS);
    assert_eq!(web_keys("LOCA_COLOR_NAMES"), protocol::LOCA_COLORS);
    for color in protocol::LOCA_COLORS {
        let dark = format!("\n  [data-loca-color=\"{color}\"] {{ --loca-c: #");
        let light =
            format!(":root[data-theme=\"light\"] [data-loca-color=\"{color}\"] {{ --loca-c: #");
        assert!(STYLES_CSS.contains(&dark), "no dark shade for {color}");
        assert!(STYLES_CSS.contains(&light), "no light shade for {color}");
    }
}
