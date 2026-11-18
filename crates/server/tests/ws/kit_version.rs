//! The agent kit version a live connection sends (`?kit_version=`), on the
//! real binary: it shows on that seat's roster `Member` and on the Building
//! list entry of the connected member (Lobby included); anything that is not
//! a short version is ignored and kept nowhere (not in a frame, a list, the
//! log or the database); a browser connection has none; it goes away with the
//! connection; and it never opens a door or changes a role.
use super::*;

const MASTER: &str = "kit-version-master-key";
const ROOM: &str = "kit-table";

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn boot(dir: &std::path::Path) -> (u16, ServerGuard) {
    let (port, guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", dir.join("kit.db").to_str().unwrap().to_string()),
            ("REQUIRE_INVITE", "1".to_string()),
            ("LEGACY_WS_QUERY_AUTH", "0".to_string()),
            // The join lines are logged at info.
            ("RUST_LOG", "info".to_string()),
            (
                SERVER_LOG_FILE,
                dir.join("server.log").to_str().unwrap().to_string(),
            ),
        ],
    )
    .await;
    create_test_room(port, MASTER, ROOM).await;
    (port, guard)
}

/// Admit a member and return its membership key.
async fn admit_key(base: &str, name: &str, kind: &str) -> String {
    let member: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name, "kind": kind }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    member["token"].as_str().unwrap().to_string()
}

async fn admin_list(base: &str, path: &str) -> Vec<Value> {
    reqwest::Client::new()
        .get(format!("{base}{path}"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

fn entry<'a>(list: &'a [Value], name: &str) -> Option<&'a Value> {
    list.iter().find(|row| row["name"] == name)
}

/// Poll a Master list until `pred` holds for `name`'s entry.
async fn until(base: &str, path: &str, name: &str, pred: impl Fn(Option<&Value>) -> bool) {
    tokio::time::timeout(ASYNC_STATE_HANG_GUARD, async {
        loop {
            let list = admin_list(base, path).await;
            if pred(entry(&list, name)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        panic!("timed out waiting on {path} for {name}");
    });
}

/// An agent's loca socket with its invite, `extra` appended to the query.
async fn seat(port: u16, invite: &str, name: &str, extra: &str) -> Ws {
    let url = format!("ws://127.0.0.1:{port}/ws?room={ROOM}&name={name}&type=agent{extra}");
    let mut ws =
        connect_ws_protocols(url, &["loca.v1".into(), format!("loca.room.{invite}")]).await;
    let _ = wait_for(&mut ws, |frame| frame["t"] == "history").await;
    ws
}

async fn lobby(port: u16, key: &str, extra: &str) -> Ws {
    let mut ws = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws?{extra}"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await;
    let _ = wait_for(&mut ws, |frame| frame["t"] == "lobby_ready").await;
    ws
}

fn kit(row: Option<&Value>) -> Option<&str> {
    row.and_then(|row| row.get("kit_version"))
        .and_then(Value::as_str)
}

#[tokio::test]
async fn a_valid_kit_version_shows_on_the_seat_and_in_the_building_list() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _server) = boot(dir.path()).await;
    let base = format!("http://127.0.0.1:{port}");
    let invite = invite_for(&base, MASTER, ROOM, "kit-agent").await;
    let mut agent = seat(port, &invite, "kit-agent", "&kit_version=0.15.0").await;

    // The seat's own roster frame and the REST roster carry it.
    let frame = wait_for(&mut agent, |frame| {
        frame["t"] == "members"
            && kit(entry(frame["members"].as_array().unwrap(), "kit-agent")) == Some("0.15.0")
    })
    .await;
    assert_listed_frame(&frame);
    let members = admin_list(&base, &format!("/rooms/{ROOM}/members")).await;
    assert_eq!(kit(entry(&members, "kit-agent")), Some("0.15.0"));
    // The Building list entry of the connected member.
    until(&base, "/residents", "kit-agent", |row| {
        kit(row) == Some("0.15.0")
    })
    .await;

    // A member waiting in the Lobby, seated nowhere, shows it too.
    let key = admit_key(&base, "kit-lobby", "agent").await;
    let _lobby = lobby(port, &key, "kit_version=0.14.2").await;
    until(&base, "/residents", "kit-lobby", |row| {
        kit(row) == Some("0.14.2") && row.is_some_and(|r| r["online"] == true)
    })
    .await;

    // It is logged as a version, beside the join.
    let log = std::fs::read_to_string(dir.path().join("server.log")).unwrap();
    assert!(
        log.lines()
            .any(|line| line.contains("ws join") && line.contains("0.15.0")),
        "the seat's join line names its kit version"
    );
}

#[tokio::test]
async fn a_value_that_is_not_a_short_version_is_ignored_and_kept_nowhere() {
    let dir = tempfile::tempdir().unwrap();
    let (port, server) = boot(dir.path()).await;
    let base = format!("http://127.0.0.1:{port}");
    // Each carries a marker the log and the database are searched for.
    let refused = [
        // too long
        format!("1.2.3-kitmarkera{}", "a".repeat(40)),
        // control characters (a newline, a bell)
        "1.2.3-kitmarkerb%0A".to_string(),
        "1.2.3-kitmarkerc%07".to_string(),
        // not a version
        "kitmarkerd".to_string(),
        "v1.2.3-kitmarkere".to_string(),
        "1.2-kitmarkerf".to_string(),
        "1.2.3-kitmarkerg%3Cscript%3E".to_string(),
    ];
    let key = admit_key(&base, "kit-odd", "agent").await;
    let invite = invite_for(&base, MASTER, ROOM, "kit-odd").await;
    for value in &refused {
        let extra = format!("&kit_version={value}");
        let mut agent = seat(port, &invite, "kit-odd", &extra).await;
        let frame = wait_for(&mut agent, |frame| frame["t"] == "members").await;
        let me = entry(frame["members"].as_array().unwrap(), "kit-odd");
        assert!(me.is_some(), "{value}: seated");
        assert_eq!(kit(me), None, "{value}: no version on the seat");
        let _lobby = lobby(port, &key, &format!("kit_version={value}")).await;
        let members = admin_list(&base, &format!("/rooms/{ROOM}/members")).await;
        assert_eq!(kit(entry(&members, "kit-odd")), None, "{value}");
        let residents = admin_list(&base, "/residents").await;
        assert_eq!(entry(&residents, "kit-odd").unwrap()["online"], true);
        assert_eq!(kit(entry(&residents, "kit-odd")), None, "{value}");
    }

    // Nothing of them was logged or written: stop the server so the database
    // and its write-ahead log are complete on disk, then search every file.
    drop(server);
    wait_for_server_shutdown(port).await;
    let mut pending = vec![dir.path().to_path_buf()];
    let mut searched = 0;
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for file in std::fs::read_dir(&path).unwrap() {
                pending.push(file.unwrap().path());
            }
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        searched += 1;
        assert!(
            !String::from_utf8_lossy(&bytes).contains("kitmarker"),
            "{} holds a refused kit version",
            path.display()
        );
    }
    assert!(searched >= 2, "the log and the database were searched");
}

#[tokio::test]
async fn a_browser_connection_has_no_kit_version() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _server) = boot(dir.path()).await;
    let base = format!("http://127.0.0.1:{port}");
    // A person signs in the way the web page does: a session from the key,
    // then the loca socket with that session and no kit version.
    let key = admit_key(&base, "kit-person", "user").await;
    let invite = invite_for(&base, MASTER, ROOM, "kit-person").await;
    let session = session_with(&base, ("x-room-token", &invite), "kit-person", Some(ROOM)).await;
    let mut person = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room={ROOM}&name=kit-person&type=user"),
        &["loca.v1".into(), format!("loca.session.{session}")],
    )
    .await;
    let frame = wait_for(&mut person, |frame| frame["t"] == "members").await;
    let me = entry(frame["members"].as_array().unwrap(), "kit-person");
    assert!(me.is_some());
    assert!(me.unwrap().get("kit_version").is_none());
    let _lobby = lobby(port, &key, "").await;
    let residents = admin_list(&base, "/residents").await;
    assert_eq!(entry(&residents, "kit-person").unwrap()["online"], true);
    assert!(entry(&residents, "kit-person")
        .unwrap()
        .get("kit_version")
        .is_none());
}

#[tokio::test]
async fn the_kit_version_goes_away_with_its_connection() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _server) = boot(dir.path()).await;
    let base = format!("http://127.0.0.1:{port}");
    let key = admit_key(&base, "kit-leaver", "agent").await;
    let invite = invite_for(&base, MASTER, ROOM, "kit-leaver").await;
    // Present in the Lobby without a version, so the member stays online.
    let _plain_lobby = lobby(port, &key, "").await;

    // A seat with a version, then closed: the Building list keeps the member
    // online through the Lobby, without the version.
    let mut agent = seat(port, &invite, "kit-leaver", "&kit_version=0.13.0").await;
    until(&base, "/residents", "kit-leaver", |row| {
        kit(row) == Some("0.13.0")
    })
    .await;
    agent.close(None).await.unwrap();
    until(&base, "/residents", "kit-leaver", |row| {
        row.is_some_and(|r| r["online"] == true) && kit(row).is_none()
    })
    .await;

    // The same seat taken again by a connection that sends none: the earlier
    // connection's version does not come back with the seat.
    let _versioned = seat(port, &invite, "kit-leaver", "&kit_version=0.13.0").await;
    until(
        &base,
        &format!("/rooms/{ROOM}/members"),
        "kit-leaver",
        |row| kit(row) == Some("0.13.0"),
    )
    .await;
    let mut plain = seat(port, &invite, "kit-leaver", "").await;
    let frame = wait_for(&mut plain, |frame| frame["t"] == "members").await;
    assert_eq!(
        kit(entry(frame["members"].as_array().unwrap(), "kit-leaver")),
        None
    );
    let members = admin_list(&base, &format!("/rooms/{ROOM}/members")).await;
    assert_eq!(kit(entry(&members, "kit-leaver")), None);

    // A Lobby socket's version goes with that socket.
    let versioned_lobby = lobby(port, &key, "kit_version=0.12.0").await;
    until(&base, "/residents", "kit-leaver", |row| {
        kit(row) == Some("0.12.0")
    })
    .await;
    drop(versioned_lobby);
    until(&base, "/residents", "kit-leaver", |row| kit(row).is_none()).await;
}

#[tokio::test]
async fn a_kit_version_never_opens_a_door_or_changes_a_role() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _server) = boot(dir.path()).await;
    let base = format!("http://127.0.0.1:{port}");
    let invite = invite_for(&base, MASTER, ROOM, "kit-plain").await;

    // No credential: refused the same with or without a version.
    for extra in ["", "&kit_version=0.16.0", "&kit_version=9999.0.0"] {
        let url = format!("ws://127.0.0.1:{port}/ws?room={ROOM}&name=kit-plain&type=agent{extra}");
        let refused = tokio_tungstenite::connect_async(url).await;
        match refused {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status(), 401, "{extra}");
            }
            other => panic!("{extra}: a socket without a credential opened: {other:?}"),
        }
    }
    // The Lobby the same.
    let refused = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/lobby/ws?kit_version=9999.0.0"
    ))
    .await;
    assert!(matches!(
        refused,
        Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == 401
    ));

    // Seated with a version: an ordinary member, no rank, no Master list.
    let mut agent = seat(port, &invite, "kit-plain", "&kit_version=9999.0.0").await;
    let frame = wait_for(&mut agent, |frame| {
        frame["t"] == "members"
            && kit(entry(frame["members"].as_array().unwrap(), "kit-plain")) == Some("9999.0.0")
    })
    .await;
    let me = entry(frame["members"].as_array().unwrap(), "kit-plain").unwrap();
    assert!(me.get("role").is_none(), "a version is no rank: {me}");
    let master_list = reqwest::Client::new()
        .get(format!("{base}/residents"))
        .header("x-room-token", invite.as_str())
        .send()
        .await
        .unwrap();
    assert!(master_list.status().is_client_error());
    // A control frame from this seat is dropped as from any member: no
    // `control` reaches the loca before this seat's next message does.
    for frame in [
        serde_json::json!({"t": "control", "cmd": "stop"}),
        serde_json::json!({"t": "send", "text": "kit-after-control"}),
    ] {
        agent
            .send(WsMessage::Text(frame.to_string().into()))
            .await
            .unwrap();
    }
    let seen = wait_for(&mut agent, |frame| {
        frame["t"] == "control"
            || (frame["t"] == "msg" && frame["message"]["text"] == "kit-after-control")
    })
    .await;
    assert_eq!(seen["t"], "msg", "a member's control went through: {seen}");
}
