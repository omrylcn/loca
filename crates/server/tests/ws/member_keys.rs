//! Membership keys and invites are stored only as hashes and ids, on the real
//! binary: after a Building has been used every way that mints, delivers,
//! claims, suspends or revokes one, no table and no file holds a key or an
//! invite except a bounded delivery copy inside its 24 hours; a value derived
//! from one (its hash, an id, a fingerprint, a scope key) opens no door;
//! and suspension and both revoke doors hold across a restart, when the
//! server knows the keys only by their hashes.

use super::*;
use sha2::Digest;

const ROOT: &str = "member-keys-fixture-root";
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
const SETUP_ACKED: &str = "cl_MemberKeysSetupAckedaaaaaaaaaaaaaaaaaaaaaaa";
const SETUP_OPEN: &str = "cl_MemberKeysSetupOpenbbbbbbbbbbbbbbbbbbbbbbbb";

fn persistent(db: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![
        ("DB_PATH", db.to_string_lossy().into_owned()),
        ("REQUIRE_INVITE", "1".into()),
    ]
}

fn sha256(value: &str) -> String {
    format!("{:x}", sha2::Sha256::digest(value.as_bytes()))
}

/// The key ledger's fingerprint: the prefix and four characters.
fn fingerprint(secret: &str) -> String {
    let (prefix, rest) = secret.split_once('_').unwrap();
    format!("{prefix}_{}…", &rest[..4])
}

async fn send(
    method: reqwest::Method,
    base: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<Value>,
) -> (u16, Value) {
    let mut request = reqwest::Client::new().request(method, format!("{base}{path}"));
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn admit_key(base: &str, name: &str) -> String {
    let (status, member) = send(
        reqwest::Method::POST,
        base,
        "/members",
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "name": name, "kind": "agent" })),
    )
    .await;
    assert_eq!(status, 201, "{member}");
    member["token"].as_str().unwrap().to_string()
}

async fn invite(base: &str, room: &str, name: &str) -> String {
    let (status, invite_key) = send(
        reqwest::Method::POST,
        base,
        &format!("/rooms/{room}/invites"),
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, 200, "{invite_key}");
    invite_key["token"].as_str().unwrap().to_string()
}

async fn call_in(base: &str, room: &str, name: &str, reissue: bool) -> String {
    let (status, called) = send(
        reqwest::Method::POST,
        base,
        &format!("/rooms/{room}/call"),
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "name": name, "reissue": reissue })),
    )
    .await;
    assert_eq!(status, 201, "{called}");
    called["token"].as_str().unwrap().to_string()
}

/// A join request approved and collected: the requester's key, and whether
/// it was acknowledged.
async fn joined(base: &str, name: &str, acknowledge: bool) -> String {
    let (_, created) = send(
        reqwest::Method::POST,
        base,
        "/join-requests",
        &[],
        Some(serde_json::json!({ "name": name, "kind": "agent" })),
    )
    .await;
    let id = created["request_id"].as_str().unwrap().to_string();
    let secret = created["request_secret"].as_str().unwrap().to_string();
    let (status, _) = send(
        reqwest::Method::POST,
        base,
        &format!("/join-requests/{id}/approve"),
        &[("x-admin-token", ROOT)],
        None,
    )
    .await;
    assert_eq!(status, 200);
    let (status, collected) = send(
        reqwest::Method::POST,
        base,
        &format!("/join-requests/{id}/bootstrap"),
        &[("x-join-secret", &secret)],
        None,
    )
    .await;
    assert_eq!(status, 201, "{collected}");
    if acknowledge {
        let (status, _) = send(
            reqwest::Method::POST,
            base,
            &format!("/join-requests/{id}/ack"),
            &[("x-join-secret", &secret)],
            None,
        )
        .await;
        assert_eq!(status, 200);
    }
    collected["davet"].as_str().unwrap().to_string()
}

async fn claim(base: &str, invite_key: &str, setup: &str) -> String {
    let (status, claimed) = send(
        reqwest::Method::POST,
        base,
        "/membership/claim",
        &[("x-room-token", invite_key), ("x-claim-id", setup)],
        None,
    )
    .await;
    assert_eq!(status, 201, "{claimed}");
    claimed["membership_token"].as_str().unwrap().to_string()
}

async fn whoami(base: &str, header: (&str, &str)) -> u16 {
    send(reqwest::Method::GET, base, "/whoami", &[header], None)
        .await
        .0
}

async fn lobby_opens(port: u16, key: &str) -> bool {
    let mut request = format!("ws://127.0.0.1:{port}/lobby/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.membership.{key}")).unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.is_ok()
}

/// Every `table.column` of the database at `db` holding `needle`, over every
/// table `sqlite_master` lists and every column each has: a column added
/// later that stores a key is found too.
fn columns_holding(db: &std::path::Path, needle: &str) -> Vec<String> {
    let conn = rusqlite::Connection::open(db).unwrap();
    let tables: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let mut found = Vec::new();
    for table in tables {
        let mut statement = conn.prepare(&format!("SELECT * FROM \"{table}\"")).unwrap();
        let names: Vec<String> = statement.column_names().map(str::to_string).collect();
        let mut rows = statement.query([]).unwrap();
        while let Some(row) = rows.next().unwrap() {
            for (i, column) in names.iter().enumerate() {
                let bytes = match row.get_ref(i).unwrap() {
                    rusqlite::types::ValueRef::Text(bytes)
                    | rusqlite::types::ValueRef::Blob(bytes) => bytes.to_vec(),
                    _ => continue,
                };
                if bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                    found.push(format!("{table}.{column}"));
                }
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The files of the database at `db` (itself, `-wal`, `-shm`) holding
/// `needle` byte for byte.
fn files_holding(db: &std::path::Path, needle: &str) -> Vec<String> {
    ["", "-wal", "-shm"]
        .into_iter()
        .filter(|suffix| {
            std::fs::read(format!("{}{suffix}", db.display()))
                .map(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
                .unwrap_or(false)
        })
        .map(|suffix| format!("db{suffix}"))
        .collect()
}

fn snapshot(db: &std::path::Path, destination: &std::path::Path) -> std::path::PathBuf {
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/storage_snapshot.py");
    let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
    let backup = std::process::Command::new(python)
        .arg(script)
        .arg("backup")
        .arg(db)
        .arg(destination)
        .arg("--server-stopped")
        .output()
        .unwrap();
    assert!(
        backup.status.success(),
        "{}",
        String::from_utf8_lossy(&backup.stderr)
    );
    destination.join("loca.sqlite3")
}

/// Plan item 2.6: no raw credential in the database. A Building is used
/// every way that mints, hands over, claims, uses, re-issues, suspends or
/// revokes a membership key or an invite. Then every column of every table,
/// the database file, its WAL and a snapshot are searched for each key and
/// invite, whole and without the part the ledger shows. Only the bounded
/// delivery copies are found, each in its own column: a join request's key
/// collected and not acknowledged, a claimed key not acknowledged, an invite
/// minted for delivery and not used, and the copy of an invite that ended
/// (re-issued, suspended, revoked) until the next sweep (every 15 seconds)
/// or start clears it; an ended invite opens nothing, its copy included.
/// Once their 24 hours have passed, the next
/// start clears them and nothing is found anywhere, while every key still
/// works.
#[tokio::test]
async fn no_key_or_invite_is_stored_anywhere_but_a_delivery_copy_inside_its_window() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("keys.sqlite3");
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    for room in ["parlor", "ops"] {
        create_test_room(port, ROOT, room).await;
    }
    send(
        reqwest::Method::POST,
        &base,
        "/admission-stock",
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "count": 2 })),
    )
    .await;
    let ada = admit_key(&base, "ada").await;
    let bea = admit_key(&base, "bea").await;
    let cem = admit_key(&base, "cem").await;
    let dan = admit_key(&base, "dan").await;
    let eve = admit_key(&base, "eve").await;
    let jay = joined(&base, "jay", false).await;
    let kim = joined(&base, "kim", true).await;
    let (status, device) = send(
        reqwest::Method::POST,
        &base,
        "/profile/credentials",
        &[("x-admin-token", &ada)],
        Some(serde_json::json!({ "label": "ada laptop" })),
    )
    .await;
    assert!(status < 300, "{device}");
    let device = device["secret"].as_str().unwrap().to_string();

    // Minted for delivery and never used: its copy is held.
    let ada_owed = invite(&base, "parlor", "ada").await;
    // Used by a session: its copy ends at once. The same invite claims a key
    // for two set-ups, one acknowledged, one not.
    let bea_used = call_in(&base, "parlor", "bea", false).await;
    let claimed_acked = claim(&base, &bea_used, SETUP_ACKED).await;
    let (status, _) = send(
        reqwest::Method::POST,
        &base,
        "/membership/claim/ack",
        &[("x-room-token", &bea_used), ("x-claim-id", SETUP_ACKED)],
        None,
    )
    .await;
    assert_eq!(status, 200);
    let claimed_open = claim(&base, &bea_used, SETUP_OPEN).await;
    let bea_session = session_with(&base, ("x-room-token", &bea_used), "bea", None).await;
    let (status, posted) = send(
        reqwest::Method::POST,
        &base,
        "/rooms/parlor/messages",
        &[("x-session-token", &bea_session)],
        Some(serde_json::json!({
            "sender": "bea", "sender_type": "agent", "text": "kept by id", "op_id": "op-bea"
        })),
    )
    .await;
    assert_eq!(status, 201, "{posted}");
    let (status, posted) = send(
        reqwest::Method::POST,
        &base,
        "/rooms/parlor/messages",
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "sender": "Master", "sender_type": "user", "text": "react" })),
    )
    .await;
    assert_eq!(status, 201, "{posted}");
    let message = posted["id"].as_u64().unwrap();
    let (status, _) = send(
        reqwest::Method::POST,
        &base,
        &format!("/rooms/parlor/messages/{message}/reactions"),
        &[("x-session-token", &bea_session)],
        Some(serde_json::json!({ "emoji": "✓", "active": true, "reactor": "bea", "reactor_type": "agent" })),
    )
    .await;
    assert_eq!(status, 200);
    // Re-issued: the first invite ends, the fresh one is owed.
    let cem_first = call_in(&base, "ops", "cem", false).await;
    let cem_fresh = call_in(&base, "ops", "cem", true).await;
    // Ended by a suspension (then resumed) and by revoking the membership.
    let dan_invite = invite(&base, "ops", "dan").await;
    let eve_invite = invite(&base, "ops", "eve").await;
    let (status, _) = send(
        reqwest::Method::POST,
        &base,
        &format!("/members/{}/suspension", member_ref(&dan)),
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "suspended": true })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        reqwest::Method::POST,
        &base,
        &format!("/members/{}/suspension", member_ref(&dan)),
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "suspended": false })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        reqwest::Method::DELETE,
        &base,
        &format!("/members/{}", member_ref(&eve)),
        &[("x-admin-token", ROOT)],
        None,
    )
    .await;
    assert!(status < 300);
    // ada's Lobby is sent her owed invite.
    assert!(lobby_opens(port, &ada).await);
    drop(guard);
    wait_for_server_shutdown(port).await;

    let secrets: Vec<(&str, &String, Vec<&str>)> = vec![
        ("ada's key", &ada, vec![]),
        ("bea's key", &bea, vec![]),
        ("cem's key", &cem, vec![]),
        ("dan's key", &dan, vec![]),
        ("eve's key", &eve, vec![]),
        ("jay's key", &jay, vec!["join_requests.mb_token"]),
        ("kim's key", &kim, vec![]),
        ("ada's device key", &device, vec![]),
        ("an acknowledged claimed key", &claimed_acked, vec![]),
        (
            "a claimed key not yet acknowledged",
            &claimed_open,
            vec!["membership_claims.delivery_token"],
        ),
        (
            "ada's owed invite",
            &ada_owed,
            vec!["invites.delivery_token"],
        ),
        ("bea's used invite", &bea_used, vec![]),
        (
            "cem's invite ended by the re-issue",
            &cem_first,
            vec!["invites.delivery_token"],
        ),
        (
            "cem's fresh invite",
            &cem_fresh,
            vec!["invites.delivery_token"],
        ),
        (
            "dan's invite ended by suspension",
            &dan_invite,
            vec!["invites.delivery_token"],
        ),
        (
            "eve's invite ended with her membership",
            &eve_invite,
            vec!["invites.delivery_token"],
        ),
        ("bea's invite session", &bea_session, vec![]),
    ];
    let kept = snapshot(&db, &dir.path().join("snapshot-1"));
    for (what, secret, allowed) in &secrets {
        let tail = &secret[7..];
        for needle in [secret.as_str(), tail] {
            assert_eq!(
                columns_holding(&db, needle),
                *allowed,
                "{what}: only its delivery copy may hold it"
            );
            if allowed.is_empty() {
                assert_eq!(files_holding(&db, needle), Vec::<String>::new(), "{what}");
                assert_eq!(files_holding(&kept, needle), Vec::<String>::new(), "{what}");
            }
        }
    }

    // A day later (the copies' times are moved back, nothing else): the
    // start clears every copy past its window and every copy of an ended
    // invite, and then nothing is found anywhere. Every key still works.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        for sql in [
            "UPDATE join_requests SET decided_at = decided_at - ?1",
            "UPDATE membership_claims SET claimed_at = claimed_at - ?1",
            "UPDATE invites SET issued_at = issued_at - ?1",
        ] {
            conn.execute(sql, [DAY_MS + 60_000]).unwrap();
        }
    }
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    for (what, key) in [
        ("ada", &ada),
        ("jay", &jay),
        ("kim", &kim),
        ("ada's device", &device),
        ("the claimed keys", &claimed_open),
    ] {
        assert_eq!(
            whoami(&base, ("x-room-token", key)).await,
            200,
            "{what} after the restart"
        );
    }
    assert_eq!(whoami(&base, ("x-room-token", &eve)).await, 401);
    assert_eq!(
        rest_can(&base, "parlor", &[("x-room-token", &ada_owed)]).await,
        200
    );
    assert_eq!(
        rest_can(&base, "ops", &[("x-room-token", &cem_fresh)]).await,
        200
    );
    assert_eq!(
        rest_can(&base, "ops", &[("x-room-token", &cem_first)]).await,
        401
    );
    drop(guard);
    wait_for_server_shutdown(port).await;
    let kept = snapshot(&db, &dir.path().join("snapshot-2"));
    for (what, secret, _) in &secrets {
        for needle in [secret.as_str(), &secret[7..]] {
            assert_eq!(columns_holding(&db, needle), Vec::<String>::new(), "{what}");
            assert_eq!(files_holding(&db, needle), Vec::<String>::new(), "{what}");
            assert_eq!(files_holding(&kept, needle), Vec::<String>::new(), "{what}");
        }
    }
}

/// What the server now stores about a key or an invite (its hash, its ids,
/// its fingerprint) and the scope keys it names a sender by are not
/// credentials. Asked after a restart, when the server knows the member and
/// the invite only by what it stored: each is refused at every door that
/// takes a key or an invite, and refusing changes nothing.
#[tokio::test]
async fn a_stored_hash_id_fingerprint_or_scope_key_opens_no_door_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("derived.sqlite3");
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, "parlor").await;
    let key = admit_key(&base, "ada").await;
    let invite_key = invite(&base, "parlor", "ada").await;
    drop(guard);
    wait_for_server_shutdown(port).await;

    let stored: Vec<String> = {
        let conn = rusqlite::Connection::open(&db).unwrap();
        let member: (String, String, String, String, String) = conn
            .query_row(
                "SELECT id, secret_hash, principal_id, credential_id, fingerprint FROM members",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        let invite: (String, String, String) = conn
            .query_row(
                "SELECT id, secret_hash, fingerprint FROM invites",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(member.1, sha256(&key));
        assert_eq!(member.4, fingerprint(&key));
        assert_eq!(invite.1, sha256(&invite_key));
        vec![
            member.1.clone(),
            member.0.clone(),
            member.2,
            member.3,
            member.4,
            invite.1.clone(),
            invite.0.clone(),
            invite.2,
            format!("mb:{}", member.0),
            format!("member:{}", member.1),
            format!("dv:{}", invite.0),
            format!("invite:{}", invite.1),
        ]
    };

    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    // The key and the invite themselves still work: nothing below is refused
    // for want of a live membership.
    assert_eq!(whoami(&base, ("x-room-token", &key)).await, 200);
    assert_eq!(
        rest_can(&base, "parlor", &[("x-room-token", &invite_key)]).await,
        200
    );
    let count = |path: &'static str| {
        let base = base.clone();
        async move {
            send(
                reqwest::Method::GET,
                &base,
                path,
                &[("x-admin-token", ROOT)],
                None,
            )
            .await
            .1
            .as_array()
            .map(Vec::len)
        }
    };
    let before = (
        count("/members").await,
        count("/rooms/parlor/invites").await,
    );
    for value in &stored {
        for header in ["x-room-token", "x-admin-token"] {
            assert_eq!(
                whoami(&base, (header, value)).await,
                401,
                "{header} {value}"
            );
            assert_eq!(
                rest_can(&base, "parlor", &[(header, value)]).await,
                401,
                "{header} {value}"
            );
        }
        assert_eq!(
            send(
                reqwest::Method::GET,
                &base,
                "/profile",
                &[("x-admin-token", value)],
                None
            )
            .await
            .0,
            401,
            "{value}"
        );
        assert!(!lobby_opens(port, value).await, "{value}");
        assert!(!ws_protocol_can(port, "parlor", value).await, "{value}");
        for (path, expected) in [
            ("/sessions", 401),
            ("/membership/claim", 401),
            ("/rooms/parlor/release", 401),
        ] {
            let (status, body) = send(
                reqwest::Method::POST,
                &base,
                path,
                &[("x-room-token", value), ("x-claim-id", SETUP_OPEN)],
                Some(serde_json::json!({ "name": "ada", "kind": "agent" })),
            )
            .await;
            assert_eq!(status, expected, "{path} {value}: {body}");
        }
    }
    // A management URL takes the ids; a hash names nothing.
    for path in [
        format!("/members/{}", sha256(&key)),
        format!("/rooms/parlor/invites/{}", sha256(&invite_key)),
    ] {
        let (status, _) = send(
            reqwest::Method::DELETE,
            &base,
            &path,
            &[("x-admin-token", ROOT)],
            None,
        )
        .await;
        assert_eq!(status, 404, "{path}");
    }
    // Nothing changed.
    assert_eq!(
        (
            count("/members").await,
            count("/rooms/parlor/invites").await
        ),
        before
    );
    assert_eq!(whoami(&base, ("x-room-token", &key)).await, 200);
    assert_eq!(
        rest_can(&base, "parlor", &[("x-room-token", &invite_key)]).await,
        200
    );
    let claims: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row("SELECT count(*) FROM membership_claims", [], |r| r.get(0))
        .unwrap();
    assert_eq!(claims, 0, "no claim was recorded");
}

/// Suspension and both revoke doors, decided before a restart, still hold
/// after it, when the server finds members and invites only by their hashes:
/// a suspended member's key and old invite stay refused (the key's holder is
/// told it is suspended, and only that holder), and resuming
/// (after the restart) brings back the key but not the invite; door A ends
/// the key, the keys its invites claimed and its invite; door B ends the one
/// credential and leaves the member's other key and its invite working.
#[tokio::test]
async fn suspension_and_both_revoke_doors_hold_across_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("doors.sqlite3");
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    create_test_room(port, ROOT, "parlor").await;
    let paused = admit_key(&base, "paused").await;
    let paused_invite = invite(&base, "parlor", "paused").await;
    let gone = admit_key(&base, "gone").await;
    let gone_invite = invite(&base, "parlor", "gone").await;
    let gone_claimed = claim(&base, &gone_invite, SETUP_ACKED).await;
    let kept = admit_key(&base, "kept").await;
    let kept_invite = invite(&base, "parlor", "kept").await;
    let kept_claimed = claim(&base, &kept_invite, SETUP_ACKED).await;
    let (status, _) = send(
        reqwest::Method::POST,
        &base,
        &format!("/members/{}/suspension", member_ref(&paused)),
        &[("x-admin-token", ROOT)],
        Some(serde_json::json!({ "suspended": true })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, _) = send(
        reqwest::Method::DELETE,
        &base,
        &format!("/members/{}", member_ref(&gone)),
        &[("x-admin-token", ROOT)],
        None,
    )
    .await;
    assert!(status < 300);
    // Door B: the original key of `kept` is revoked with its claimed key.
    let (_, listed) = send(
        reqwest::Method::GET,
        &base,
        "/profile/credentials",
        &[("x-admin-token", &kept_claimed)],
        None,
    )
    .await;
    let original = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["label"] == "Member access")
        .expect("the original key's credential")["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = send(
        reqwest::Method::DELETE,
        &base,
        &format!("/profile/credentials/{original}"),
        &[("x-admin-token", &kept_claimed)],
        None,
    )
    .await;
    assert!(status < 300, "door B: {status}");
    drop(guard);
    wait_for_server_shutdown(port).await;

    for start in ["first", "second"] {
        let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
        let base = format!("http://127.0.0.1:{port}");
        for (what, key) in [
            ("the suspended key", &paused),
            ("the revoked member's key", &gone),
            ("its claimed key", &gone_claimed),
            ("the key revoked at door B", &kept),
        ] {
            // The holder of a suspended member's own key is told so (403);
            // every other refused key is indistinguishable (401).
            let suspended = what == "the suspended key";
            let resumed = start == "second" && suspended;
            assert_eq!(
                whoami(&base, ("x-room-token", key)).await,
                match (resumed, suspended) {
                    (true, _) => 200,
                    (false, true) => 403,
                    (false, false) => 401,
                },
                "{start} start: {what}"
            );
            assert_eq!(
                lobby_opens(port, key).await,
                resumed,
                "{start} start: {what}"
            );
        }
        for (what, invite_key) in [
            ("the suspended member's invite", &paused_invite),
            ("the revoked member's invite", &gone_invite),
        ] {
            assert_eq!(
                rest_can(&base, "parlor", &[("x-room-token", invite_key)]).await,
                401,
                "{start} start: {what}"
            );
        }
        assert_eq!(
            whoami(&base, ("x-room-token", &kept_claimed)).await,
            200,
            "{start} start: door B leaves the member's other key"
        );
        assert_eq!(
            rest_can(&base, "parlor", &[("x-room-token", &kept_invite)]).await,
            200,
            "{start} start: door B leaves the invite"
        );
        if start == "first" {
            let (status, _) = send(
                reqwest::Method::POST,
                &base,
                &format!("/members/{}/suspension", member_ref(&paused)),
                &[("x-admin-token", ROOT)],
                Some(serde_json::json!({ "suspended": false })),
            )
            .await;
            assert_eq!(status, 200, "resumed after the restart");
            assert_eq!(whoami(&base, ("x-room-token", &paused)).await, 200);
        }
        drop(guard);
        wait_for_server_shutdown(port).await;
    }
}
