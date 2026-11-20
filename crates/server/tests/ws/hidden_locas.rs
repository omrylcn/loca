//! A direct message and a group loca are invisible to everyone who may not enter
//! them: at every door that takes a loca id, the refusal such a caller gets
//! is byte-for-byte the answer the same caller gets for an id that names
//! nothing. PRINCIPLES "Birebir": "A Smaster does not even see that they
//! exist" (and "A higher layer contains the ones below it" for everyone
//! below the Smaster). PRINCIPLES "Groups": "outsiders do not learn that a
//! group loca exists"; "The rule is the same at every door (sidebar, REST,
//! WS, search)".
//!
//! A direct message id is a hash of the two principal ids, which a Smaster can read
//! from `GET /profiles`, so any difference between "a real direct message you may
//! not enter" and "nothing" tells it who talks to whom.
use super::*;

const ROOT: &str = "hidden-root-credential";

/// Every `/rooms/:id…` route the router serves, as `(METHOD, path)`, read
/// from the router (`authority_matrix::served_routes`) so a new route is
/// covered as it lands.
fn loca_routes() -> Vec<(String, String)> {
    let out: Vec<(String, String)> = super::authority_matrix::served_routes()
        .into_iter()
        .filter(|(_, path)| path == "/rooms/:id" || path.starts_with("/rooms/:id/"))
        .collect();
    // A reader that matches nothing would pass every test below vacuously
    // (the router's path syntax changed once under it).
    assert!(
        out.len() > 40,
        "only {} loca routes read from router.rs",
        out.len()
    );
    out
}

fn concrete(path: &str, loca: &str) -> String {
    path.split('/')
        .map(|seg| match seg {
            ":id" => loca.to_string(),
            s if s.starts_with(':') => "1".to_string(),
            s => s.to_string(),
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// One answer, everything a caller can read from it: status, body, and every
/// header except the clock.
#[derive(Debug, PartialEq, Eq)]
struct Answer {
    status: u16,
    body: String,
    headers: Vec<(String, String)>,
}

fn headers_of(map: &reqwest::header::HeaderMap) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = map
        .iter()
        .filter(|(k, _)| k.as_str() != "date")
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                v.to_str().unwrap_or("?").to_string(),
            )
        })
        .collect();
    out.sort();
    out
}

async fn rest(
    client: &reqwest::Client,
    base: &str,
    method: &str,
    path: &str,
    credential: &(&'static str, String),
) -> Answer {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let mut req = client
        .request(method.clone(), format!("{base}{path}"))
        .header(credential.0, &credential.1);
    if method != reqwest::Method::GET {
        req = req.json(&serde_json::json!({}));
    }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let headers = headers_of(r.headers());
    Answer {
        status,
        body: r.text().await.unwrap_or_default(),
        headers,
    }
}

/// The WebSocket door, the credential riding in the subprotocol as a client
/// does it.
async fn ws(port: u16, room: &str, credential: &(&'static str, String)) -> Answer {
    let url = format!("ws://127.0.0.1:{port}/ws?room={room}&name=probe&type=agent");
    let mut request = url.into_client_request().unwrap();
    let prefix = match credential.0 {
        "x-session-token" => "loca.session.",
        "x-admin-token" => "loca.admin.",
        _ => "loca.room.",
    };
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, {prefix}{}", credential.1)).unwrap(),
    );
    match tokio_tungstenite::connect_async(request).await {
        Ok((mut ws, response)) => {
            let _ = ws.close(None).await;
            Answer {
                status: response.status().as_u16(),
                body: String::new(),
                headers: Vec::new(),
            }
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => Answer {
            status: response.status().as_u16(),
            body: response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default(),
            headers: headers_of(response.headers()),
        },
        Err(other) => panic!("websocket handshake failed outside HTTP: {other}"),
    }
}

struct Building {
    _guard: ServerGuard,
    _dir: tempfile::TempDir,
    port: u16,
    base: String,
    client: reqwest::Client,
    dm: String,
    group_loca: String,
    /// Who may not enter the direct message, and how they present themselves.
    dm_outsiders: Vec<(&'static str, (&'static str, String))>,
    /// Who may not enter the group loca.
    group_outsiders: Vec<(&'static str, (&'static str, String))>,
}

async fn json_ok(r: reqwest::Response, what: &str) -> Value {
    let status = r.status();
    let text = r.text().await.unwrap_or_default();
    assert!(status.is_success(), "fixture {what}: {status} {text}");
    serde_json::from_str(&text).unwrap_or(Value::String(text))
}

/// A closed Building: the Master; Smaster `deputy`, who founds group `squad`
/// and its loca `squad-atelier`; Smaster `runnerup`, outside the group; members
/// `opal` (human) and `envoy` (agent) seated in `atelier`, `envoy` also in the
/// group, and `opal` opens a direct message with `envoy`; `idler`, a member seated
/// nowhere; `loca-care`, the caretaker seated in Care; `lodger`, who holds
/// only an invite for `atelier`.
async fn building() -> Building {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("hidden.db");
    let (port, guard) = spawn_server_env(
        ROOT,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            ("REQUIRE_INVITE", "1".into()),
            ("LOCA_AGENT_ROOM", "iye".into()),
            ("RESERVED_LOCA", "iye".into()),
            ("LOCA_CARETAKERS", "loca-care".into()),
            ("LEGACY_WS_QUERY_AUTH", "0".into()),
            ("RATE_LIMIT", "100000".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let root = |req: reqwest::RequestBuilder| req.header("x-admin-token", ROOT);
    let session = |header: &'static str, key: String, name: &'static str, kind: &'static str| {
        let client = client.clone();
        let base = base.clone();
        async move {
            let v = json_ok(
                client
                    .post(format!("{base}/sessions"))
                    .header(header, key)
                    .json(&serde_json::json!({"name": name, "kind": kind}))
                    .send()
                    .await
                    .unwrap(),
                name,
            )
            .await;
            v["session_token"].as_str().unwrap().to_string()
        }
    };

    // Two Smasters, each signed in with the code the Master issued.
    let mut smaster_key = std::collections::HashMap::new();
    let mut smaster_session = std::collections::HashMap::new();
    for name in ["deputy", "runnerup"] {
        let code = json_ok(
            root(client.post(format!("{base}/smasters")))
                .json(&serde_json::json!({"name": name}))
                .send()
                .await
                .unwrap(),
            "smaster",
        )
        .await["token"]
            .as_str()
            .unwrap()
            .to_string();
        smaster_session.insert(
            name,
            session("x-admin-token", code.clone(), name, "user").await,
        );
        smaster_key.insert(name, code);
    }

    let mut keys = std::collections::HashMap::new();
    for (name, kind) in [
        ("opal", "user"),
        ("envoy", "agent"),
        ("idler", "user"),
        ("loca-care", "agent"),
        ("lodger", "agent"),
    ] {
        let m = json_ok(
            root(client.post(format!("{base}/members")))
                .json(&serde_json::json!({"name": name, "kind": kind}))
                .send()
                .await
                .unwrap(),
            name,
        )
        .await;
        keys.insert(name, m["token"].as_str().unwrap().to_string());
    }
    let profiles = json_ok(
        root(client.get(format!("{base}/profiles")))
            .send()
            .await
            .unwrap(),
        "profiles",
    )
    .await;
    let envoy_id = profiles
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["display_name"] == "envoy")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();

    json_ok(
        root(client.post(format!("{base}/rooms")))
            .json(&serde_json::json!({"name": "atelier"}))
            .send()
            .await
            .unwrap(),
        "create atelier",
    )
    .await;
    let mut lodger_invite = String::new();
    for name in ["opal", "envoy", "lodger"] {
        let d = json_ok(
            root(client.post(format!("{base}/rooms/atelier/invites")))
                .json(&serde_json::json!({"name": name}))
                .send()
                .await
                .unwrap(),
            "seat in atelier",
        )
        .await;
        if name == "lodger" {
            lodger_invite = d["token"].as_str().unwrap().to_string();
        }
    }
    json_ok(
        root(client.post(format!("{base}/rooms/iye/invites")))
            .json(&serde_json::json!({"name": "loca-care"}))
            .send()
            .await
            .unwrap(),
        "caretaker invite",
    )
    .await;

    let opal = session("x-room-token", keys["opal"].clone(), "opal", "user").await;
    let idler = session("x-room-token", keys["idler"].clone(), "idler", "user").await;
    let care = session(
        "x-room-token",
        keys["loca-care"].clone(),
        "loca-care",
        "agent",
    )
    .await;
    let lodger = session("x-room-token", lodger_invite.clone(), "lodger", "agent").await;

    // The group and its loca, founded by `deputy`; `envoy` is a member.
    let deputy = ("x-session-token", smaster_session["deputy"].clone());
    let group = json_ok(
        client
            .post(format!("{base}/groups"))
            .header(deputy.0, &deputy.1)
            .json(&serde_json::json!({"name": "squad"}))
            .send()
            .await
            .unwrap(),
        "group",
    )
    .await["id"]
        .as_str()
        .unwrap()
        .to_string();
    json_ok(
        client
            .put(format!("{base}/groups/{group}/members/{envoy_id}"))
            .header(deputy.0, &deputy.1)
            .send()
            .await
            .unwrap(),
        "group member",
    )
    .await;
    json_ok(
        client
            .post(format!("{base}/rooms"))
            .header(deputy.0, &deputy.1)
            .json(&serde_json::json!({"name": "squad-atelier", "group": group}))
            .send()
            .await
            .unwrap(),
        "group loca",
    )
    .await;

    // The direct message: opal opens one with envoy, the agent it sits with.
    let dm = json_ok(
        client
            .post(format!("{base}/birebirs"))
            .header("x-session-token", &opal)
            .json(&serde_json::json!({"name": "envoy"}))
            .send()
            .await
            .unwrap(),
        "dm",
    )
    .await["birebir"]["room"]
        .as_str()
        .unwrap()
        .to_string();

    let s = |token: &String| ("x-session-token", token.clone());
    let dm_outsiders = vec![
        ("smaster (session)", deputy.clone()),
        (
            "smaster (own key)",
            ("x-admin-token", smaster_key["deputy"].clone()),
        ),
        ("other smaster", s(&smaster_session["runnerup"])),
        ("lobby member (session)", s(&idler)),
        (
            "lobby member (membership key)",
            ("x-room-token", keys["idler"].clone()),
        ),
        ("caretaker", s(&care)),
        ("invite holder (session)", s(&lodger)),
        (
            "invite holder (invite)",
            ("x-room-token", lodger_invite.clone()),
        ),
    ];
    let group_outsiders = vec![
        ("other smaster (session)", s(&smaster_session["runnerup"])),
        (
            "other smaster (own key)",
            ("x-admin-token", smaster_key["runnerup"].clone()),
        ),
        ("member outside the group", s(&opal)),
        ("lobby member", s(&idler)),
        ("caretaker", s(&care)),
        ("invite holder (session)", s(&lodger)),
        ("invite holder (invite)", ("x-room-token", lodger_invite)),
    ];
    Building {
        _guard: guard,
        _dir: dir,
        port,
        base,
        client,
        dm,
        group_loca: "squad-atelier".into(),
        dm_outsiders,
        group_outsiders,
    }
}

/// Ids that name nothing: one shaped like a direct message id, one like a loca name.
fn nothing_ids(dm: &str) -> [String; 2] {
    [
        format!("bb-{}", "0".repeat(dm.len() - 3)),
        "nothing-here".to_string(),
    ]
}

async fn rest_differences(
    b: &Building,
    loca: &str,
    outsiders: &[(&'static str, (&'static str, String))],
) -> Vec<String> {
    let routes = loca_routes();
    assert!(
        routes.len() >= 70,
        "the route reader found only {} /rooms/:id routes",
        routes.len()
    );
    let mut differences = Vec::new();
    for (who, credential) in outsiders {
        for (method, path) in &routes {
            let real = rest(
                &b.client,
                &b.base,
                method,
                &concrete(path, loca),
                credential,
            )
            .await;
            assert!(
                matches!(real.status, 401 | 403 | 404),
                "{who} {method} {path} on a loca it may not enter must be refused: {real:?}"
            );
            for nothing in nothing_ids(&b.dm) {
                let none = rest(
                    &b.client,
                    &b.base,
                    method,
                    &concrete(path, &nothing),
                    credential,
                )
                .await;
                if real != none {
                    differences.push(format!(
                        "{who} {method} {path}: real {} {:?} {:?} / nothing ({nothing}) {} {:?} {:?}",
                        real.status, real.body, real.headers, none.status, none.body, none.headers
                    ));
                }
            }
        }
    }
    differences
}

async fn ws_differences(
    b: &Building,
    loca: &str,
    outsiders: &[(&'static str, (&'static str, String))],
) -> Vec<String> {
    let mut differences = Vec::new();
    for (who, credential) in outsiders {
        let real = ws(b.port, loca, credential).await;
        assert!(
            matches!(real.status, 401 | 403 | 404),
            "{who} must not open a socket into {loca}: {real:?}"
        );
        for nothing in nothing_ids(&b.dm) {
            let none = ws(b.port, &nothing, credential).await;
            if real != none {
                differences.push(format!(
                    "{who} /ws: real {} {:?} {:?} / nothing ({nothing}) {} {:?} {:?}",
                    real.status, real.body, real.headers, none.status, none.body, none.headers
                ));
            }
        }
    }
    differences
}

#[tokio::test]
async fn a_dm_answers_strangers_exactly_as_nothing_does_on_every_door() {
    let b = building().await;
    let mut differences = rest_differences(&b, &b.dm, &b.dm_outsiders).await;
    differences.extend(ws_differences(&b, &b.dm, &b.dm_outsiders).await);
    assert!(
        differences.is_empty(),
        "a direct message's existence leaks in {} answers:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

#[tokio::test]
async fn a_group_loca_answers_outsiders_exactly_as_nothing_does_on_every_door() {
    let b = building().await;
    let mut differences = rest_differences(&b, &b.group_loca, &b.group_outsiders).await;
    differences.extend(ws_differences(&b, &b.group_loca, &b.group_outsiders).await);
    assert!(
        differences.is_empty(),
        "a group loca's existence leaks in {} answers:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
