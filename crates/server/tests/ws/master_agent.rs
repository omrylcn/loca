//! The Master Agent rank (owner's decisions, 2026-11-18): an automation the
//! Master appoints. The Building switch is off by default and only the
//! Master turns it on or off; while it is off no Master Agent key is issued
//! and every Master Agent credential is refused with one sentence. A Master
//! Agent enters no loca it did not open — not one on a Ground floor, not a
//! direct message, not Care, not another loca of a floor it was given.
//!
//! PRINCIPLES does not name the rank yet; the owner changes that text. The
//! lines it touches are quoted in the pull request.
use super::*;

const MASTER: &str = "ma-master-key";
pub(super) const OFF: &str = "Master Agent is turned off in this Building";

pub(super) async fn send(
    method: reqwest::Method,
    url: String,
    header: Option<(&str, &str)>,
    body: Option<Value>,
) -> (u16, String) {
    let mut req = reqwest::Client::new().request(method, url);
    if let Some((k, v)) = header {
        req = req.header(k, v);
    }
    if let Some(body) = body {
        req = req.json(&body);
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    (status, r.text().await.unwrap_or_default())
}

pub(super) fn json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

pub(super) struct Building {
    pub(super) _guard: ServerGuard,
    pub(super) _dir: tempfile::TempDir,
    pub(super) port: u16,
    pub(super) base: String,
    pub(super) master: String,
    pub(super) smaster: String,
    pub(super) floor: String,
    pub(super) floor_loca: String,
    pub(super) dm: String,
}

impl Building {
    pub(super) fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub(super) async fn as_master(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, String) {
        send(
            method,
            self.url(path),
            Some(("x-session-token", &self.master)),
            body,
        )
        .await
    }

    pub(super) async fn setting(&self, on: bool) -> (u16, String) {
        self.as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": on, "floors": [self.floor] })),
        )
        .await
    }

    pub(super) async fn appoint(&self, name: &str) -> (u16, Value) {
        let (status, text) = self
            .as_master(
                reqwest::Method::POST,
                "/keys",
                Some(serde_json::json!({ "kind": "master_agent", "name": name })),
            )
            .await;
        (status, json(&text))
    }

    pub(super) async fn ws_status(&self, room: &str, session: &str) -> u16 {
        let mut request = format!(
            "ws://127.0.0.1:{}/ws?room={room}&name=probe&type=agent",
            self.port
        )
        .into_client_request()
        .unwrap();
        request.headers_mut().insert(
            "Sec-WebSocket-Protocol",
            HeaderValue::from_str(&format!("loca.v1, loca.session.{session}")).unwrap(),
        );
        match tokio_tungstenite::connect_async(request).await {
            Ok((mut ws, response)) => {
                let _ = ws.close(None).await;
                response.status().as_u16()
            }
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                response.status().as_u16()
            }
            Err(other) => panic!("websocket handshake failed outside HTTP: {other}"),
        }
    }
}

pub(super) async fn session(base: &str, header: (&str, &str), name: &str, kind: &str) -> String {
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/sessions"),
        Some(header),
        Some(serde_json::json!({ "name": name, "kind": kind })),
    )
    .await;
    assert!(status < 300, "session for {name}: {status} {text}");
    json(&text)["session_token"].as_str().unwrap().to_string()
}

/// A closed Building with a Ground-floor loca, Care, a direct message and a
/// Smaster's floor with its own loca.
pub(super) async fn building() -> Building {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("master-agent.db");
    let (port, guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let root = ("x-admin-token", MASTER);
    let (_, pairing) = send(
        reqwest::Method::POST,
        format!("{base}/pairings?ttl_hours=1"),
        Some(root),
        None,
    )
    .await;
    let code = json(&pairing)["pairing_code"].as_str().unwrap().to_string();
    let master = session(&base, ("x-pairing-code", &code), "", "user").await;
    let (_, sm) = send(
        reqwest::Method::POST,
        format!("{base}/smasters"),
        Some(root),
        Some(serde_json::json!({ "name": "deputy" })),
    )
    .await;
    let sm_key = json(&sm)["token"].as_str().unwrap().to_string();
    let smaster = session(&base, ("x-admin-token", &sm_key), "deputy", "user").await;

    // Members: a human and an agent who sit together on the Ground floor.
    let mut keys = Vec::new();
    for (name, kind) in [("opal", "user"), ("envoy", "agent")] {
        let (status, text) = send(
            reqwest::Method::POST,
            format!("{base}/members"),
            Some(root),
            Some(serde_json::json!({ "name": name, "kind": kind })),
        )
        .await;
        assert_eq!(status, 201, "{text}");
        keys.push(json(&text)["token"].as_str().unwrap().to_string());
    }
    create_test_room(port, MASTER, "ground").await;
    for name in ["opal", "envoy"] {
        let (status, text) = send(
            reqwest::Method::POST,
            format!("{base}/rooms/ground/call"),
            Some(root),
            Some(serde_json::json!({ "name": name })),
        )
        .await;
        assert_eq!(status, 201, "{text}");
    }
    let opal = session(&base, ("x-room-token", &keys[0]), "opal", "user").await;
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/birebirs"),
        Some(("x-session-token", &opal)),
        Some(serde_json::json!({ "name": "envoy" })),
    )
    .await;
    assert!(status < 300, "dm: {status} {text}");
    let dm = json(&text)["birebir"]["room"].as_str().unwrap().to_string();

    // A Smaster's floor with its own loca: the floor the Master gives the
    // Master Agent; the loca on it is the Smaster's, not the Master Agent's.
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/groups"),
        Some(("x-session-token", &smaster)),
        Some(serde_json::json!({ "name": "project" })),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    let floor = json(&text)["id"].as_str().unwrap().to_string();
    let (status, text) = send(
        reqwest::Method::POST,
        format!("{base}/rooms"),
        Some(("x-session-token", &smaster)),
        Some(serde_json::json!({ "name": "project-room", "group": floor })),
    )
    .await;
    assert_eq!(status, 201, "{text}");

    Building {
        _guard: guard,
        _dir: dir,
        port,
        base,
        master,
        smaster,
        floor,
        floor_loca: "project-room".into(),
        dm,
    }
}

#[tokio::test]
async fn the_switch_is_off_by_default_and_only_the_master_turns_it() {
    let b = building().await;
    let (status, text) = b
        .as_master(
            reqwest::Method::GET,
            "/building/settings/master-agent",
            None,
        )
        .await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["on"], false, "off by default");
    assert_eq!(
        json(&text)["limits"],
        serde_json::json!({
            "locas_per_floor": 20,
            "locas_per_hour": 10,
            "admissions_per_hour": 30,
            "live_agents": 50,
        }),
        "the default limits"
    );

    // The Master adjusts a limit; the ones it leaves out keep their value.
    let (status, text) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": false, "limits": { "live_agents": 5 } })),
        )
        .await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["limits"]["live_agents"], 5);
    assert_eq!(json(&text)["limits"]["locas_per_floor"], 20);
    for bad in [
        serde_json::json!({ "live_agents": 0 }),
        serde_json::json!({ "live_agents": 1001 }),
        serde_json::json!({ "live_agents": "many" }),
        serde_json::json!({ "seats": 3 }),
    ] {
        let (status, text) = b
            .as_master(
                reqwest::Method::PUT,
                "/building/settings/master-agent",
                Some(serde_json::json!({ "on": false, "limits": bad })),
            )
            .await;
        assert_eq!(status, 400, "{bad}: {text}");
    }
    let (_, text) = b
        .as_master(
            reqwest::Method::GET,
            "/building/settings/master-agent",
            None,
        )
        .await;
    assert_eq!(
        json(&text)["limits"]["live_agents"],
        5,
        "a refused change changes nothing"
    );

    // While off, no Master Agent key is issued.
    let (status, v) = b.appoint("automaton").await;
    assert_eq!(status, 403, "{v}");

    // A Smaster neither sees nor changes the switch, nor appoints.
    let sm = Some(("x-session-token", b.smaster.as_str()));
    for (method, path, body) in [
        (
            reqwest::Method::GET,
            "/building/settings/master-agent",
            None,
        ),
        (
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true })),
        ),
        (reqwest::Method::GET, "/master-agents", None),
        (
            reqwest::Method::POST,
            "/keys",
            Some(serde_json::json!({ "kind": "master_agent", "name": "rogue" })),
        ),
    ] {
        let (status, text) = send(method.clone(), b.url(path), sm, body).await;
        assert_eq!(status, 403, "a Smaster: {method} {path}: {text}");
    }

    // A floor that is not a live floor of this Building is refused.
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "floors": ["grp_nosuchfloor00"] })),
        )
        .await;
    assert_eq!(status, 400);

    // The Master turns it on; the change is journaled with who and when.
    let (status, text) = b.setting(true).await;
    assert_eq!(status, 200, "{text}");
    let v = json(&text);
    assert_eq!(v["on"], true);
    assert_eq!(v["floors"][0], b.floor.as_str());
    assert_eq!(v["log"][0]["value"]["on"], true);
    assert_eq!(v["log"][0]["by"], "Master");
    assert!(v["log"][0]["at"].as_u64().unwrap() > 0);
    let (status, text) = b.setting(false).await;
    assert_eq!(status, 200);
    let v = json(&text);
    assert_eq!(v["log"].as_array().unwrap().len(), 3, "{v}");
    assert_eq!(v["log"][0]["value"]["on"], false);
}

#[tokio::test]
async fn a_master_agent_key_is_shown_once_and_refused_while_the_switch_is_off() {
    let b = building().await;
    assert_eq!(b.setting(true).await.0, 200);
    let (status, v) = b.appoint("automaton").await;
    assert_eq!(status, 201, "{v}");
    let key = v["code"].as_str().unwrap().to_string();
    assert!(key.starts_with("ma_"), "{key}");
    let id = v["id"].as_str().unwrap().to_string();

    // The Master's list names it, who appointed it and when; never the key.
    let (status, text) = b
        .as_master(reqwest::Method::GET, "/master-agents", None)
        .await;
    assert_eq!(status, 200);
    assert!(!text.contains(&key), "the key is never listed");
    let row = &json(&text)[0];
    assert_eq!(row["name"], "automaton");
    assert_eq!(row["id"], id.as_str());
    assert!(row["appointed_by"].as_str().unwrap().starts_with("pr_"));
    let (_, ledger) = b.as_master(reqwest::Method::GET, "/keys", None).await;
    assert!(!ledger.contains(&key), "the ledger keeps no key");

    // It is an agent principal with its own rank.
    let ma = Some(("x-admin-token", key.as_str()));
    let (status, text) = send(reqwest::Method::GET, b.url("/rooms"), ma, None).await;
    assert_eq!(status, 200, "{text}");
    let ma_session = session(&b.base, ("x-admin-token", &key), "automaton", "agent").await;
    let (status, text) = send(
        reqwest::Method::GET,
        b.url("/whoami"),
        Some(("x-session-token", &ma_session)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["admin"], false, "no Building authority");

    // Off: the key, a session taken with it, and a new session are refused
    // with the one sentence; no new key is issued. Nothing else changes.
    assert_eq!(b.setting(false).await.0, 200);
    for header in [
        ("x-admin-token", key.as_str()),
        ("x-session-token", ma_session.as_str()),
    ] {
        let (status, text) = send(reqwest::Method::GET, b.url("/rooms"), Some(header), None).await;
        assert_eq!((status, text.as_str()), (403, OFF), "{}", header.0);
    }
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/sessions"),
        Some(("x-admin-token", &key)),
        Some(serde_json::json!({ "name": "automaton", "kind": "agent" })),
    )
    .await;
    assert_eq!((status, text.as_str()), (403, OFF));
    assert_eq!(b.appoint("second").await.0, 403);
    let (status, _) = send(
        reqwest::Method::GET,
        b.url("/rooms/ground/messages"),
        Some(("x-session-token", &b.smaster)),
        None,
    )
    .await;
    assert_eq!(status, 200, "a Smaster is not touched by the switch");

    // On again: the same credentials work again.
    assert_eq!(b.setting(true).await.0, 200);
    for header in [
        ("x-admin-token", key.as_str()),
        ("x-session-token", ma_session.as_str()),
    ] {
        let (status, text) = send(reqwest::Method::GET, b.url("/rooms"), Some(header), None).await;
        assert_eq!(status, 200, "{}: {text}", header.0);
    }

    // Revoked: dead for good, whatever the switch says.
    let (status, text) = b
        .as_master(
            reqwest::Method::DELETE,
            &format!("/master-agents/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{text}");
    for header in [
        ("x-admin-token", key.as_str()),
        ("x-session-token", ma_session.as_str()),
    ] {
        let (status, _) = send(reqwest::Method::GET, b.url("/rooms"), Some(header), None).await;
        assert_eq!(status, 401, "{} after revocation", header.0);
    }
    let (status, _) = b
        .as_master(
            reqwest::Method::DELETE,
            &format!("/master-agents/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 404, "revoked once");
}

#[tokio::test]
async fn a_master_agent_enters_no_loca_it_did_not_open() {
    let b = building().await;
    assert_eq!(b.setting(true).await.0, 200);
    let (_, v) = b.appoint("automaton").await;
    let key = v["code"].as_str().unwrap().to_string();
    let ma = session(&b.base, ("x-admin-token", &key), "automaton", "agent").await;
    let headers = [
        ("x-session-token", ma.as_str()),
        ("x-admin-token", key.as_str()),
    ];

    // Lists no loca.
    for header in headers {
        let (status, text) = send(reqwest::Method::GET, b.url("/rooms"), Some(header), None).await;
        assert_eq!(status, 200, "{text}");
        assert_eq!(json(&text), serde_json::json!([]), "{}", header.0);
    }

    // The door: a Ground-floor loca, Care, a direct message, and another
    // loca on the very floor the Master designated for it.
    for room in [
        "ground",
        "lobby",
        "iye",
        b.dm.as_str(),
        b.floor_loca.as_str(),
    ] {
        for header in headers {
            for path in ["messages", "members", "settings", "notes"] {
                let (status, text) = send(
                    reqwest::Method::GET,
                    b.url(&format!("/rooms/{room}/{path}")),
                    Some(header),
                    None,
                )
                .await;
                assert!(
                    matches!(status, 401 | 403 | 404),
                    "{room}/{path} read by {}: {status} {text}",
                    header.0
                );
            }
            let (status, text) = send(
                reqwest::Method::POST,
                b.url(&format!("/rooms/{room}/messages")),
                Some(header),
                Some(serde_json::json!({ "text": "hello", "sender": "automaton", "sender_type": "agent" })),
            )
            .await;
            assert!(
                matches!(status, 401 | 403 | 404),
                "{room} post by {}: {status} {text}",
                header.0
            );
        }
        let ws = b.ws_status(room, &ma).await;
        assert_ne!(ws, 101, "{room}: the socket is refused");
    }

    // A floor loca and a direct message answer as an id that names nothing.
    for room in [b.dm.as_str(), b.floor_loca.as_str()] {
        let seen = send(
            reqwest::Method::GET,
            b.url(&format!("/rooms/{room}/messages")),
            Some(("x-session-token", &ma)),
            None,
        )
        .await;
        let ghost = send(
            reqwest::Method::GET,
            b.url("/rooms/ghost-00000000000000000000000000000000/messages"),
            Some(("x-session-token", &ma)),
            None,
        )
        .await;
        assert_eq!(seen, ghost, "{room} is hidden from a Master Agent");
    }

    // Everything else is refused: by the fence, or by the route (a loca on
    // the Ground floor, a lead where it opened nothing).
    for (method, path, body) in [
        (
            reqwest::Method::POST,
            "/groups",
            Some(serde_json::json!({ "name": "mine" })),
        ),
        (reqwest::Method::GET, "/groups", None),
        (
            reqwest::Method::POST,
            "/rooms",
            Some(serde_json::json!({ "name": "mine" })),
        ),
        (
            reqwest::Method::POST,
            "/members",
            Some(serde_json::json!({ "name": "bot", "kind": "agent" })),
        ),
        (reqwest::Method::GET, "/join-requests", None),
        (reqwest::Method::GET, "/profiles", None),
        (
            reqwest::Method::POST,
            "/rooms/ground/call",
            Some(serde_json::json!({ "name": "envoy" })),
        ),
        (
            reqwest::Method::POST,
            "/rooms/ground/lead",
            Some(serde_json::json!({ "lead": "envoy" })),
        ),
        (reqwest::Method::GET, "/keys", None),
        (
            reqwest::Method::GET,
            "/building/settings/master-agent",
            None,
        ),
    ] {
        let (status, text) = send(
            method.clone(),
            b.url(path),
            Some(("x-session-token", &ma)),
            body,
        )
        .await;
        assert!(
            matches!(status, 401 | 403 | 404),
            "{method} {path}: {status} {text}"
        );
    }

    // Nobody opens a direct message with it, and it is offered to nobody.
    for opener in [b.master.as_str(), b.smaster.as_str()] {
        let (status, text) = send(
            reqwest::Method::POST,
            b.url("/birebirs"),
            Some(("x-session-token", opener)),
            Some(serde_json::json!({ "name": "automaton" })),
        )
        .await;
        assert_eq!(status, 404, "a direct message with a Master Agent: {text}");
        let (status, text) = send(
            reqwest::Method::GET,
            b.url("/birebirs/candidates"),
            Some(("x-session-token", opener)),
            None,
        )
        .await;
        assert_eq!(status, 200, "{text}");
        assert!(!text.contains("automaton"), "not a candidate: {text}");
    }
}
