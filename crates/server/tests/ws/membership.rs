//! Building membership, invites, residents, and identity seats.

use super::*;

/// Reading is as private as writing: with a room token set, an outsider must
/// not be able to read history, notes, tasks or the roster. (Regression for a
/// live leak: writes were gated, reads were not.)
#[tokio::test]
async fn reads_require_membership_too() {
    let (port, _guard) = spawn_server_env("adm", &[("ROOM_TOKEN", "join".into())]).await;
    create_test_room(port, "adm", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Seed something worth stealing (as a member).
    client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-room-token", "join")
        .json(&serde_json::json!({ "sender": "a", "sender_type": "user", "text": "secret plan" }))
        .send()
        .await
        .unwrap();

    // Outsider: every read is refused.
    for path in [
        "/rooms",
        "/rooms/general/messages",
        "/rooms/general/members",
        "/rooms/general/notes",
        "/rooms/general/tasks",
        "/rooms/general/goals",
        "/rooms/general/waits",
        "/rooms/general/mode",
        "/rooms/general/settings",
        "/rooms/general/search?q=secret",
    ] {
        let r = client.get(format!("{base}{path}")).send().await.unwrap();
        assert_eq!(r.status(), 401, "outsider must not read {path}");
    }

    // Public by design: the shell, discovery, and session exchange.
    for path in ["/", "/health"] {
        let r = client.get(format!("{base}{path}")).send().await.unwrap();
        assert!(r.status().is_success(), "{path} must stay reachable");
    }

    // Member reads fine, with the room token…
    let ok = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-room-token", "join")
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), 200);

    // …and with a session token alone (it proves membership).
    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", "join")
        .json(&serde_json::json!({ "name": "reader", "kind": "user" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let with_sess = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-session-token", sess["session_token"].as_str().unwrap())
        .send()
        .await
        .unwrap();
    assert_eq!(with_sess.status(), 200);
}

/// Membership and an invite are different acts, and the difference is the point.
///
/// Belonging to the building creates an identity: heavy, rare, done from a
/// terminal. An invite seats an existing member in one loca: light, frequent,
/// done from the UI. Leaving a loca must never cost the building — otherwise
/// every call-in starts from scratch again, which is the whole problem.
#[tokio::test]
async fn membership_outlives_the_locas_you_are_called_into() {
    let (port, _guard) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    create_test_room(port, "MASTER", "sb-dev").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let residents = |c: reqwest::Client, base: String| async move {
        c.get(format!("{base}/residents"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .json::<Value>()
            .await
            .unwrap()
    };

    // Calling in an unknown name does NOT quietly create an identity — the
    // building refuses and says what to do instead. Knowing someone and
    // seating them are different acts.
    let unknown = client
        .post(format!("{base}/rooms/sb-dev/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "mobile-dev" }))
        .send()
        .await
        .unwrap();
    assert_eq!(unknown.status(), 404, "an unknown name is not called in");

    // Admit them (the founding act), then the call-in is one click.
    admit(&base, "MASTER", "mobile-dev", "agent").await;
    let called = client
        .post(format!("{base}/rooms/sb-dev/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "mobile-dev" }))
        .send()
        .await
        .unwrap();
    assert!(
        called.status().is_success(),
        "calling a member in gives them an invite"
    );

    let r = residents(client.clone(), base.clone()).await;
    let me = r
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "mobile-dev")
        .unwrap();
    assert_eq!(
        me["locas"].as_array().unwrap().len(),
        1,
        "now seated in that loca"
    );

    // Take the seat away: the invite ends, the BELONGING does not. They stay a
    // resident of the building — available, seatless, one click from the next
    // call. This is the whole point of the two acts being separate.
    let inv: Value = client
        .get(format!("{base}/rooms/sb-dev/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        inv[0]["token"].is_null(),
        "list must redact the invite secret"
    );
    let token = inv[0]["id"].as_str().unwrap();
    client
        .delete(format!("{base}/rooms/sb-dev/invites/{token}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();

    let r = residents(client.clone(), base.clone()).await;
    let me = r
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["name"] == "mobile-dev")
        .expect("membership outlives the invite — still a resident");
    assert!(
        me["locas"].as_array().unwrap().is_empty(),
        "the seat is gone once the invite is revoked"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Membership is the constitution: an invite seats an EXISTING member, it never
// creates identity. "Knowing someone and inviting them to the table are not
// the same act."
// ─────────────────────────────────────────────────────────────────────────────

/// An invite for an unknown name is refused — admitting is its own act.
#[tokio::test]
async fn invite_requires_existing_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let r = reqwest::Client::new()
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "stranger" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "an invite never creates identity");
}

/// Calling in an unknown name is refused with directions, not quietly obeyed.
#[tokio::test]
async fn call_requires_existing_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let r = client
        .post(format!("{base}/rooms/general/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "stranger" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 404);
    assert!(r.text().await.unwrap().contains("admit them first"));
}

/// Issuing an invite must not grow the membership list as a side effect.
#[tokio::test]
async fn invite_does_not_create_identity() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    admit(&base, "MASTER", "cyber", "agent").await;
    let before: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let _d = invite_for(&base, "MASTER", "general", "cyber").await;
    let after: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before.len(), after.len(), "an invite is not a membership");
}

/// Admit & invite are two records, and the invite points at the membership.
#[tokio::test]
async fn admit_and_invite_creates_two_distinct_records() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let m: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "debug", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mb = m["token"].as_str().unwrap();
    assert!(mb.starts_with("mb_"), "membership is its own record");
    let inv: Value = client
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "debug" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(inv["token"].as_str().unwrap().starts_with("dv_"));
    assert_eq!(
        inv["member_id"].as_str().unwrap(),
        member_ref(mb),
        "the invite names its member, by the member's management id"
    );
    assert_eq!(
        inv["id"].as_str().unwrap(),
        invite_ref(inv["token"].as_str().unwrap())
    );
    assert!(
        inv.get("member").is_none() && !inv.to_string().contains("mb_"),
        "issuing an invite must not hand the issuer the member's key: {inv}"
    );
}

/// A session taken with alice's invite IS alice — whatever the body claims.
#[tokio::test]
async fn session_identity_comes_from_invited_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = invite_for(&base, "MASTER", "general", "alice").await;
    let sess: Value = client
        .post(format!("{base}/sessions"))
        .header("x-room-token", &d)
        .json(&serde_json::json!({ "name": "bob", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        sess["name"], "alice",
        "the invite says who you are, not the body"
    );
    // And what they post is signed as alice.
    let msg: Value = client
        .post(format!("{base}/rooms/general/messages"))
        .header("x-room-token", &d)
        .header("x-session-token", sess["session_token"].as_str().unwrap())
        .json(&serde_json::json!({ "sender": "bob", "sender_type": "agent", "text": "hi" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        msg["sender"], "alice",
        "identity cannot be spoofed by the body"
    );
}

/// The resident list starts from membership and nothing else adds a name:
/// a connected stranger with a building key is not a resident.
#[tokio::test]
async fn resident_list_contains_only_building_members() {
    let (port, _g) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    admit(&base, "MASTER", "known", "agent").await;
    // A stranger connects with the building key — seated, but not a resident.
    let url =
        format!("ws://127.0.0.1:{port}/ws?room=general&name=stranger&type=agent&token=building");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let _ = wait_for(&mut ws, |frame| frame["t"] == "history").await;
    let r: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/residents"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        r.iter().any(|x| x["name"] == "known"),
        "members are residents"
    );
    assert!(
        !r.iter().any(|x| x["name"] == "stranger"),
        "a connection is not a membership"
    );
}

/// A member with no invite anywhere is in the lobby: visible and callable, but
/// in no conversation.
#[tokio::test]
async fn member_without_seat_appears_in_the_lobby() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    admit(&base, "MASTER", "idle", "agent").await;
    let r: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/residents"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let me = r
        .iter()
        .find(|x| x["name"] == "idle")
        .expect("in the building lobby, waiting");
    assert!(me["locas"].as_array().unwrap().is_empty());
    assert_eq!(me["online"], false);
}

/// One member, one loca, one live invite — the second ask says so.
#[tokio::test]
async fn duplicate_invite_for_same_member_and_room_is_rejected() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let _first = invite_for(&base, "MASTER", "general", "cyber").await;
    let again = reqwest::Client::new()
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409, "one live invite per member per loca");
}

/// Old databases carry invites with no member link. Boot binds each to the
/// membership wearing its name — or mints one marked as migrated — so nobody
/// who was already let in loses their seat when the constitution tightens.
#[tokio::test]
async fn legacy_invites_migrate_to_memberships() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy.db").to_string_lossy().to_string();
    // A pre-link database: the invites table has no `member` column at all.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE invites (
                token TEXT PRIMARY KEY, room TEXT NOT NULL, name TEXT NOT NULL,
                kind TEXT NOT NULL, issued_at INTEGER NOT NULL,
                issued_by TEXT NOT NULL, revoked_at INTEGER
            );
            INSERT INTO invites VALUES
                ('dv_legacy_cyber', 'general', 'cyber', 'agent', 1, 'master', NULL);",
        )
        .unwrap();
    }
    let (port, _g) = spawn_server_env("MASTER", &[("DB_PATH", db.clone())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // The old invite still opens its loca — nobody is locked out by the upgrade.
    let r = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-room-token", "dv_legacy_cyber")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "a legacy invite still works");
    // And behind it now stands a real membership, marked as migrated.
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let m = members
        .iter()
        .find(|m| m["name"] == "cyber")
        .expect("migration admitted the legacy invitee");
    assert_eq!(m["admitted_by"], "migration:legacy-invite");
    let invs: Vec<Value> = client
        .get(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        invs[0]["member_id"], m["id"],
        "the invite is bound to the redacted membership id"
    );
    assert!(invs[0]["token"].is_null());
    assert!(m["token"].is_null());
}

// ─────────────────────────────────────────────────────────────────────────────
// One key = one seat. The seat is keyed by IDENTITY; the display name is just
// the label it wears. The same key entering twice takes its seat over — it
// cannot become two people.
// ─────────────────────────────────────────────────────────────────────────────

/// The live ghost scenario, verbatim: the operator entered as "operator",
/// then re-entered as "master" with the same admin key — and became two.
#[tokio::test]
async fn same_admin_key_two_names_one_seat() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let first_url =
        format!("ws://127.0.0.1:{port}/ws?room=general&name=operator&type=user&admin=MASTER");
    let (mut first, _) = tokio_tungstenite::connect_async(&first_url).await.unwrap();
    let _ = wait_for(&mut first, |v| v["t"] == "history").await;
    let second_url =
        format!("ws://127.0.0.1:{port}/ws?room=general&name=master&type=user&admin=MASTER");
    let (mut second, _) = tokio_tungstenite::connect_async(&second_url).await.unwrap();
    let _ = wait_for(&mut second, |v| v["t"] == "history").await;
    // The first connection is told to step aside — same key, same seat.
    let evicted = wait_for(&mut first, |v| v["t"] == "evicted").await;
    assert_eq!(evicted["t"], "evicted");
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/general/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(members.len(), 1, "one key, one seat");
    assert_eq!(
        members[0]["name"], "master",
        "the seat wears the newest name"
    );
}

/// Same invite, spoofed query name → one seat under the invited identity.
#[tokio::test]
async fn same_invite_two_names_one_seat() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let d = invite_for(&base, "MASTER", "general", "cyber").await;
    let (mut a, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=cyber&type=agent&token={d}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut a, |v| v["t"] == "history").await;
    let (mut b, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=cyber2&type=agent&token={d}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut b, |v| v["t"] == "history").await;
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/general/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(members.len(), 1, "one invite, one seat");
    assert_eq!(
        members[0]["name"], "cyber",
        "an invite's recorded identity overrides query-string aliases"
    );
}

/// Two DIFFERENT members picking the same display name are still two people.
#[tokio::test]
async fn two_different_members_same_display_name_two_seats() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "general").await;
    let base = format!("http://127.0.0.1:{port}");
    let d1 = invite_for(&base, "MASTER", "general", "g1").await;
    let d2 = invite_for(&base, "MASTER", "general", "g2").await;
    let (mut a, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=guest&type=agent&token={d1}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut a, |v| v["t"] == "history").await;
    let (mut b, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=general&name=guest&type=agent&token={d2}"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut b, |v| v["t"] == "history").await;
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/general/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        members.len(),
        2,
        "different identities, different seats — the name is a label"
    );
}

/// The master's seat key is per-loca: sitting in many rooms is normal.
#[tokio::test]
async fn master_in_multiple_rooms_keeps_one_seat_each() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "parlor-a").await;
    create_test_room(port, "MASTER", "parlor-b").await;
    let base = format!("http://127.0.0.1:{port}");
    let (mut a, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=parlor-a&name=master&type=user&admin=MASTER"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut a, |v| v["t"] == "history").await;
    let (mut b, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=parlor-b&name=master&type=user&admin=MASTER"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut b, |v| v["t"] == "history").await;
    for room in ["parlor-a", "parlor-b"] {
        let members: Vec<Value> = reqwest::Client::new()
            .get(format!("{base}/rooms/{room}/members"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(members.len(), 1, "one master seat in {room}");
    }
}

/// Capacity counts identities: one key under many names is still one person;
/// only a NEW identity can overflow the loca.
#[tokio::test]
async fn capacity_counts_identities_not_names() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "packed").await;
    let base = format!("http://127.0.0.1:{port}");
    // Seven members take the seven seats.
    let mut held = Vec::new();
    for i in 1..=7 {
        let d = invite_for(&base, "MASTER", "packed", &format!("m{i}")).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}/ws?room=packed&name=m{i}&type=agent&token={d}"
        ))
        .await
        .unwrap();
        let _ = wait_for(&mut ws, |v| v["t"] == "history").await;
        held.push((ws, d));
    }
    // A seated identity re-enters under a new name: no overflow, same seat.
    let d1 = held[0].1.clone();
    let (mut renamed, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=packed&name=m1-new&type=agent&token={d1}"
    ))
    .await
    .unwrap();
    let frame = wait_for(&mut renamed, |v| v["t"] == "history" || v["t"] == "control").await;
    assert_eq!(
        frame["t"], "history",
        "your own seat is always yours to retake"
    );
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/packed/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        members.len(),
        7,
        "still seven — a rename is not an eighth person"
    );
    // A genuinely new identity is turned away at mint (Model A: seven invites,
    // no eighth). The rename above did not consume an extra seat, so the seven
    // invites are still all that exist and the eighth mint is refused.
    admit(&base, "MASTER", "m8", "agent").await;
    let eighth = reqwest::Client::new()
        .post(format!("{base}/rooms/packed/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "m8" }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        eighth.status(),
        409,
        "the eighth identity is refused — no free seat"
    );
}

/// DELETE /rooms/A/invites/<tokenB> must not revoke loca B's invite through
/// loca A's door — the URL's loca and the token's loca must agree.
#[tokio::test]
async fn revoke_invite_checks_the_loca() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    create_test_room(port, "MASTER", "real").await;
    create_test_room(port, "MASTER", "other").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = invite_for(&base, "MASTER", "real", "cyber").await;
    let invites: Vec<Value> = client
        .get(format!("{base}/rooms/real/invites"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let management_id = invites[0]["id"].as_str().unwrap();
    assert!(invites[0]["token"].is_null());
    // Try to revoke it through the wrong loca's URL.
    let wrong = client
        .delete(format!("{base}/rooms/other/invites/{management_id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 404, "wrong loca in the URL revokes nothing");
    // The invite still works.
    assert_eq!(
        client
            .get(format!("{base}/rooms/real/messages"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // The right loca revokes it.
    let right = client
        .delete(format!("{base}/rooms/real/invites/{management_id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(right.status().is_success());
}

/// An agent can say "my work is done" for its own seat. No operator name is trusted
/// from a body: the invite at the door identifies exactly which membership and
/// loca may be released. The invite dies; membership returns to the lobby.
#[tokio::test]
async fn a_member_can_release_its_own_loca_into_the_lobby() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let invite = invite_for(&base, "MASTER", "parlor", "worker").await;

    let released = client
        .post(format!("{base}/rooms/parlor/release"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);

    let old_door = client
        .get(format!("{base}/rooms/parlor/messages"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap();
    assert_eq!(old_door.status(), 401, "self-release ends the loca invite");

    let residents: Vec<Value> = client
        .get(format!("{base}/residents"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let worker = residents
        .iter()
        .find(|resident| resident["name"] == "worker")
        .expect("release keeps the building membership");
    assert!(
        worker["locas"].as_array().unwrap().is_empty(),
        "no loca invitation means the member is back in lobby"
    );

    let called_back = client
        .post(format!("{base}/rooms/parlor/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap();
    assert!(
        called_back.status().is_success(),
        "lobby membership can be called back without setup"
    );
}

/// Kick stops the invite but keeps the membership: the master can call them
/// straight back in (a fresh invite), because kick is not a building expulsion.
#[tokio::test]
async fn kick_keeps_membership_so_recall_is_one_step() {
    // Invite-only mode: the invite is the ONLY door, so stopping it truly shuts
    // the loca (in open mode the door is invite-independent and this can't show).
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "parlor").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = invite_for(&base, "MASTER", "parlor", "cyber").await;
    assert_eq!(
        client
            .get(format!("{base}/rooms/parlor/messages"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    client
        .post(format!("{base}/rooms/parlor/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "kick", "name": "cyber" }))
        .send()
        .await
        .unwrap();
    // Old invite is dead.
    assert_eq!(
        client
            .get(format!("{base}/rooms/parlor/messages"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // But they are still a building member — call them back in one step.
    let again = client
        .post(format!("{base}/rooms/parlor/call"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert!(
        again.status().is_success(),
        "kick keeps membership — recall is one step"
    );
    let fresh: Value = again.json().await.unwrap();
    assert_eq!(
        client
            .get(format!("{base}/rooms/parlor/messages"))
            .header("x-room-token", fresh["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "the fresh invite opens the loca again"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// PHASE 2 — identity: member uniqueness, kind, revoke cascade, whoami.
// PRINCIPLES: membership = identity, invite = seat; when the parent dies the child dies.
// ─────────────────────────────────────────────────────────────────────────────

/// Admitting the same name twice is one member, not two — a name is one
/// identity, so "which cyber?" never arises.
#[tokio::test]
async fn admit_twice_same_name_is_one_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let first: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let second = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(second.status(), 200, "nothing new was admitted");
    let second: Value = second.json().await.unwrap();
    let mb = first["token"].as_str().unwrap();
    assert!(
        mb.starts_with("mb_"),
        "a fresh admission hands out its key once"
    );
    assert_eq!(
        second,
        serde_json::json!({
            "id": member_ref(mb),
            "name": "cyber",
            "kind": "agent",
            "already": true,
        }),
        "same name = same member, named by its id; its key is never read back out"
    );
    let all: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        all.iter().filter(|m| m["name"] == "cyber").count(),
        1,
        "one cyber"
    );
}

/// kind is part of identity — a bad kind is a bad request, not a silent 'agent'.
#[tokio::test]
async fn admit_rejects_unknown_kind() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let r = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "x", "kind": "banana" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 400, "kind must be agent or user");
}

/// Names the join-request door refuses (`valid_agent_name`): a space, a
/// non-ASCII letter, an `@`, and 65 characters.
const REFUSED_NAMES: &[&str] = &[
    "has space",
    "Ay\u{015F}e",
    "a@b",
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
];
const NAME_RULE: &str = "name must be 1-64 ASCII letters, digits, dot, dash, or underscore";

/// Direct admission and the master desk apply the join-request door's name
/// rule, with the same status and sentence, so no door admits a name the
/// others refuse. Valid names still pass every door.
#[tokio::test]
async fn every_admission_door_applies_the_join_request_name_rule() {
    let dir = tempfile::tempdir().unwrap();
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let console_port = socket.local_addr().unwrap().port();
    drop(socket);
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            (
                "DB_PATH",
                dir.path().join("names.db").to_string_lossy().into(),
            ),
            ("ADMIN_CONSOLE_PORT", console_port.to_string()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let console = format!("http://127.0.0.1:{console_port}");
    let client = reqwest::Client::new();
    let mut console_up = false;
    for _ in 0..100 {
        if client
            .get(format!("{console}/api/state"))
            .send()
            .await
            .is_ok()
        {
            console_up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(console_up, "master desk did not come up");

    // Every door and shape is tried before judging, so a failure names all
    // the admissions that got through, not only the first.
    let mut admitted = Vec::new();
    for (i, name) in REFUSED_NAMES.iter().enumerate() {
        let kind = if i % 2 == 0 { "agent" } else { "user" };
        let direct = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        let status = direct.status().as_u16();
        if (status, direct.text().await.unwrap().as_str()) != (400, NAME_RULE) {
            admitted.push(format!("POST /members {name:?} -> {status}"));
        }
        let desk = client
            .post(format!("{console}/api/members"))
            .header("x-loca-console", "1")
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        let status = desk.status().as_u16();
        if (status, desk.text().await.unwrap().as_str()) != (400, NAME_RULE) {
            admitted.push(format!("master desk {name:?} -> {status}"));
        }
        // The join-request door is unchanged and says the same sentence.
        let request = client
            .post(format!("{base}/join-requests"))
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        assert_eq!(request.status(), 400);
        assert_eq!(request.text().await.unwrap(), NAME_RULE);
    }
    assert!(
        admitted.is_empty(),
        "admitted past the name rule: {admitted:#?}"
    );
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        members
            .iter()
            .all(|m| !REFUSED_NAMES.contains(&m["name"].as_str().unwrap_or(""))),
        "a refused name left a membership behind: {members:?}"
    );

    // Valid names, at the edges of the rule, still pass both doors.
    let long_ok = "b".repeat(64);
    for name in ["Ada.Lovelace_2-x", long_ok.as_str()] {
        let direct = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": format!(" {name} "), "kind": "user" }))
            .send()
            .await
            .unwrap();
        assert_eq!(direct.status(), 201, "POST /members refused {name:?}");
        let admitted: Value = direct.json().await.unwrap();
        assert_eq!(admitted["name"], name);
    }
    let desk = client
        .post(format!("{console}/api/members"))
        .header("x-loca-console", "1")
        .json(&serde_json::json!({ "name": "desk.agent-1", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(desk.status(), 201, "the master desk refused a valid name");
}

/// The rule is applied at admission only. A member an older server admitted
/// with a name the doors now refuse keeps working: called into a loca, signed
/// in, posting under that name, and revoked.
#[tokio::test]
async fn a_member_admitted_before_the_name_rule_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-name.db");
    let env = [
        ("DB_PATH", db.to_string_lossy().to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    let legacy = "Ay\u{015F}e Kaya";
    let client = reqwest::Client::new();
    let membership = {
        let (port, guard) = spawn_server_env("MASTER", &env).await;
        create_test_room(port, "MASTER", "parlor").await;
        let base = format!("http://127.0.0.1:{port}");
        let admitted: Value = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": "legacy-seed", "kind": "user" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        drop(guard);
        wait_for_server_shutdown(port).await;
        admitted["token"].as_str().unwrap().to_string()
    };
    // Model the rows an older server wrote when it admitted the name directly.
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE members SET name = ?1 WHERE name = 'legacy-seed'",
            [legacy],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE principals SET display_name = ?1 WHERE display_name = 'legacy-seed'",
            [legacy],
        )
        .unwrap();
    drop(connection);

    let (port, _guard) = spawn_server_env("MASTER", &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let whoami: Value = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", &membership)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(whoami["name"], legacy, "{whoami}");
    // Asking to admit that name again finds the member; the rule is for a
    // name being admitted now, and it must not turn this into a refusal.
    let again = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": legacy, "kind": "user" }))
        .send()
        .await
        .unwrap();
    assert!(
        again.status().is_success(),
        "re-admitting a member admitted before the rule: {}",
        again.status()
    );
    let again: Value = again.json().await.unwrap();
    assert_eq!(again["name"], legacy, "{again}");
    let invite = invite_for(&base, "MASTER", "parlor", legacy).await;
    let session = session_with(&base, ("x-room-token", &invite), legacy, None).await;
    let posted = client
        .post(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", &session)
        .json(&serde_json::json!({ "sender": legacy, "sender_type": "user", "text": "still here" }))
        .send()
        .await
        .unwrap();
    assert!(posted.status().is_success(), "{}", posted.status());
    let history: Vec<Value> = client
        .get(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|m| m["sender"] == legacy && m["text"] == "still here"),
        "{history:?}"
    );
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = members.iter().find(|m| m["name"] == legacy).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revoked = client
        .delete(format!("{base}/members/{id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success(), "{}", revoked.status());
}

/// `all` in any letter case: a message's `target` of `all`, or `@all` in its
/// text, calls everyone at the table, so a person named so cannot be
/// addressed alone.
const ADDRESS_WORDS: &[&str] = &["all", "All", "ALL", "aLl", " all "];
const ADDRESS_RULE: &str = "\"all\" is an address that calls everyone, not a name";

/// Start a server with the loopback master desk, and wait until it answers.
async fn spawn_with_master_desk(env: &[(&str, String)]) -> (String, String, ServerGuard) {
    let socket = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let console_port = socket.local_addr().unwrap().port();
    drop(socket);
    let mut env = env.to_vec();
    env.push(("ADMIN_CONSOLE_PORT", console_port.to_string()));
    let (port, guard) = spawn_server_env("MASTER", &env).await;
    let console = format!("http://127.0.0.1:{console_port}");
    let client = reqwest::Client::new();
    let mut console_up = false;
    for _ in 0..100 {
        if client
            .get(format!("{console}/api/state"))
            .send()
            .await
            .is_ok()
        {
            console_up = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(console_up, "master desk did not come up");
    (format!("http://127.0.0.1:{port}"), console, guard)
}

/// Every admission door refuses `all` as a new name, in any letter case, with
/// the same status and sentence, and leaves no membership or request behind.
#[tokio::test]
async fn every_admission_door_refuses_the_address_word_all() {
    let dir = tempfile::tempdir().unwrap();
    let (base, console, _guard) = spawn_with_master_desk(&[(
        "DB_PATH",
        dir.path().join("address.db").to_string_lossy().into(),
    )])
    .await;
    let client = reqwest::Client::new();

    // Every door and casing is tried before judging, so a failure names all
    // the admissions that got through, not only the first.
    let mut admitted = Vec::new();
    for (i, name) in ADDRESS_WORDS.iter().enumerate() {
        let kind = if i % 2 == 0 { "agent" } else { "user" };
        let request = client
            .post(format!("{base}/join-requests"))
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        let status = request.status().as_u16();
        if (status, request.text().await.unwrap().as_str()) != (400, ADDRESS_RULE) {
            admitted.push(format!("POST /join-requests {name:?} -> {status}"));
        }
        let direct = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        let status = direct.status().as_u16();
        if (status, direct.text().await.unwrap().as_str()) != (400, ADDRESS_RULE) {
            admitted.push(format!("POST /members {name:?} -> {status}"));
        }
        let desk = client
            .post(format!("{console}/api/members"))
            .header("x-loca-console", "1")
            .json(&serde_json::json!({ "name": name, "kind": kind }))
            .send()
            .await
            .unwrap();
        let status = desk.status().as_u16();
        if (status, desk.text().await.unwrap().as_str()) != (400, ADDRESS_RULE) {
            admitted.push(format!("master desk {name:?} -> {status}"));
        }
    }
    assert!(
        admitted.is_empty(),
        "admitted an address word as a name: {admitted:#?}"
    );
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        members
            .iter()
            .all(|m| !m["name"].as_str().unwrap_or("").eq_ignore_ascii_case("all")),
        "an address word left a membership behind: {members:?}"
    );
    let requests: Value = client
        .get(format!("{base}/join-requests"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        !requests.to_string().to_lowercase().contains("\"all\""),
        "an address word left a join request behind: {requests}"
    );
}

/// Guard (passes before and after the address-word refusal): only the words
/// that change who a message reaches are refused. Titles and other reserved
/// words that a rename refuses (`operator`, `master`, `lead`, `goal`, ...),
/// the caretaker names and `loca` stay ordinary names at every admission
/// door: `operator` is a real member name in running Buildings. Nobody here
/// wears these names yet (the Master is called `founder`): a name someone
/// already wears in some letter case is refused, see
/// `every_admission_door_refuses_a_name_worn_in_another_letter_case`.
#[tokio::test]
async fn titles_and_caretaker_names_are_still_admitted_at_every_door() {
    let dir = tempfile::tempdir().unwrap();
    let (base, console, _guard) = spawn_with_master_desk(&[
        (
            "DB_PATH",
            dir.path().join("titles.db").to_string_lossy().into(),
        ),
        ("LOCA_MASTER_NAME", "founder".into()),
    ])
    .await;
    // Direct admission gets a Building of its own: here the desk has
    // already admitted every name, and asking again would only find it.
    let (direct_port, _direct_guard) = spawn_server_env(
        "MASTER",
        &[
            (
                "DB_PATH",
                dir.path().join("titles-direct.db").to_string_lossy().into(),
            ),
            ("LOCA_MASTER_NAME", "founder".into()),
        ],
    )
    .await;
    let direct_base = format!("http://127.0.0.1:{direct_port}");
    let client = reqwest::Client::new();
    let names = [
        "operator",
        "master",
        "smaster",
        "lead",
        "goal",
        "everyone",
        "here",
        "admin",
        "root",
        "loca",
        "loca-care",
        "loca-dev",
    ];
    let mut refused = Vec::new();
    for name in names {
        // A join request only files a request; it creates no member, so the
        // desk below still meets a new name.
        let request = client
            .post(format!("{base}/join-requests"))
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        if request.status() != 201 {
            refused.push(format!(
                "POST /join-requests {name} -> {}",
                request.status()
            ));
        }
        let desk = client
            .post(format!("{console}/api/members"))
            .header("x-loca-console", "1")
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        if desk.status() != 201 {
            refused.push(format!("master desk {name} -> {}", desk.status()));
        }
        let direct = client
            .post(format!("{direct_base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": "user" }))
            .send()
            .await
            .unwrap();
        if direct.status() != 201 {
            refused.push(format!("POST /members {name} -> {}", direct.status()));
        }
    }
    assert!(refused.is_empty(), "refused an ordinary name: {refused:#?}");
    for building in [&base, &direct_base] {
        let members: Vec<Value> = client
            .get(format!("{building}/members"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        for name in names {
            assert!(
                members.iter().any(|m| m["name"] == name),
                "{name} is not a member of {building}: {members:?}"
            );
        }
    }
}

/// Guard (passes before and after the address-word refusal): the refusal is
/// for a name being admitted now. A member an older server stored as `all`
/// keeps working, and the admitting doors find it again instead of refusing.
#[tokio::test]
async fn a_member_named_all_before_the_refusal_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-all.db");
    let env = [
        ("DB_PATH", db.to_string_lossy().to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    let client = reqwest::Client::new();
    let membership = {
        let (port, guard) = spawn_server_env("MASTER", &env).await;
        create_test_room(port, "MASTER", "parlor").await;
        let base = format!("http://127.0.0.1:{port}");
        let admitted: Value = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": "all-seed", "kind": "agent" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        drop(guard);
        wait_for_server_shutdown(port).await;
        admitted["token"].as_str().unwrap().to_string()
    };
    // Model the rows an older server wrote when it admitted `all`.
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE members SET name = 'all' WHERE name = 'all-seed'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE principals SET display_name = 'all' WHERE display_name = 'all-seed'",
            [],
        )
        .unwrap();
    drop(connection);

    let (base, console, _guard) = spawn_with_master_desk(&env).await;
    let whoami: Value = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", &membership)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(whoami["name"], "all", "{whoami}");
    // Both admitting doors find the stored member rather than refusing it.
    let again = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "all", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert!(
        again.status().is_success(),
        "re-admitting a member stored as all: {}",
        again.status()
    );
    let again: Value = again.json().await.unwrap();
    assert_eq!(again["name"], "all", "{again}");
    // The same member, named by its id: a stored key is never handed out.
    assert_eq!(again["already"], true, "{again}");
    assert_eq!(
        again["id"],
        member_ref(&membership).as_str(),
        "a second member: {again}"
    );
    assert!(again.get("token").is_none(), "{again}");
    let desk = client
        .post(format!("{console}/api/members"))
        .header("x-loca-console", "1")
        .json(&serde_json::json!({ "name": "all", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert!(
        desk.status().is_success(),
        "the master desk refused a member stored as all: {}",
        desk.status()
    );
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        members.iter().filter(|m| m["name"] == "all").count(),
        1,
        "{members:?}"
    );

    let invite = invite_for(&base, "MASTER", "parlor", "all").await;
    let session = session_with(&base, ("x-room-token", &invite), "all", None).await;
    let posted = client
        .post(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", &session)
        .json(&serde_json::json!({ "sender": "all", "sender_type": "agent", "text": "still here" }))
        .send()
        .await
        .unwrap();
    assert!(posted.status().is_success(), "{}", posted.status());
    let history: Vec<Value> = client
        .get(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", &session)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        history
            .iter()
            .any(|m| m["sender"] == "all" && m["text"] == "still here"),
        "{history:?}"
    );
    let id = members.iter().find(|m| m["name"] == "all").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revoked = client
        .delete(format!("{base}/members/{id}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert!(revoked.status().is_success(), "{}", revoked.status());
}

/// Who already wears a name, compared without letter case: `@Bob` in a
/// message wakes `bob` too, and a `target` of `LOCA-CARE` is relayed to the
/// caretaker `loca-care` in Care with the message as its context.
const NAME_WORN_RULE: &str =
    "someone in this Building already has that name in some letter case — choose another";

/// Ask every admission door to admit `name`, and return how each answered
/// whenever it was not `want`: (status, body).
async fn doors_answering_otherwise(
    base: &str,
    console: &str,
    name: &str,
    want: (u16, &str),
) -> Vec<String> {
    let client = reqwest::Client::new();
    let mut otherwise = Vec::new();
    let answers = [
        (
            "POST /join-requests",
            client
                .post(format!("{base}/join-requests"))
                .json(&serde_json::json!({ "name": name, "kind": "agent" })),
        ),
        (
            "POST /members",
            client
                .post(format!("{base}/members"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": name, "kind": "agent" })),
        ),
        (
            "master desk",
            client
                .post(format!("{console}/api/members"))
                .header("x-loca-console", "1")
                .json(&serde_json::json!({ "name": name, "kind": "agent" })),
        ),
    ];
    for (door, request) in answers {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        if (status, body.as_str()) != want {
            otherwise.push(format!("{door} {name:?} -> {status} {body}"));
        }
    }
    otherwise
}

/// A new name that a member, the Master, a Smaster or a configured caretaker
/// already wears in another letter case is refused at every admission door,
/// with 409 and one sentence, and leaves no membership or request behind. So
/// is a rank's exact name; a caretaker's exact name is how it is admitted.
#[tokio::test]
async fn every_admission_door_refuses_a_name_worn_in_another_letter_case() {
    let dir = tempfile::tempdir().unwrap();
    // The Master keeps the default display name, `Master`.
    let (base, console, _guard) = spawn_with_master_desk(&[(
        "DB_PATH",
        dir.path().join("worn.db").to_string_lossy().into(),
    )])
    .await;
    let client = reqwest::Client::new();
    admit(&base, "MASTER", "bob", "agent").await;
    let deputy = client
        .post(format!("{base}/keys"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "kind": "smaster", "name": "Deputy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(deputy.status(), 201, "Smaster fixture");

    // `loca-care` is a configured caretaker (the default) not yet admitted.
    let mut admitted = Vec::new();
    for name in [
        "Bob",
        "BOB",
        "bOb",
        "Master",
        "master",
        "MASTER",
        "Deputy",
        "deputy",
        "LOCA-CARE",
        "Loca-Care",
    ] {
        admitted
            .extend(doors_answering_otherwise(&base, &console, name, (409, NAME_WORN_RULE)).await);
    }
    assert!(
        admitted.is_empty(),
        "admitted a name someone already wears: {admitted:#?}"
    );
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = members.iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(names, ["bob"], "a refused name left a membership behind");
    let requests: Value = client
        .get(format!("{base}/join-requests"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        requests,
        serde_json::json!({ "pending": [] }),
        "a refused name left a join request behind"
    );
}

/// Guard (passes before and after the letter-case refusal): the caretaker's
/// own exact name is admitted at every door, and asking again for an existing
/// member's exact name finds that member instead of refusing it.
#[tokio::test]
async fn the_exact_caretaker_name_and_exact_readmission_still_work() {
    let dir = tempfile::tempdir().unwrap();
    let (base, console, _guard) = spawn_with_master_desk(&[(
        "DB_PATH",
        dir.path().join("exact.db").to_string_lossy().into(),
    )])
    .await;
    let client = reqwest::Client::new();
    let request = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "loca-care", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(request.status(), 201, "a join request for the caretaker");
    let desk = client
        .post(format!("{console}/api/members"))
        .header("x-loca-console", "1")
        .json(&serde_json::json!({ "name": "loca-care", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(desk.status(), 201, "the desk admits the caretaker");
    // The desk admitted the caretaker; `bob` is new. Asking again for either
    // finds the same member and hands out no key.
    let mut ids = Vec::new();
    for (name, status) in [
        ("loca-care", 200),
        ("bob", 201),
        ("loca-care", 200),
        ("bob", 200),
    ] {
        let direct = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        assert_eq!(direct.status(), status, "POST /members {name}");
        let member: Value = direct.json().await.unwrap();
        assert_eq!(member["name"], name);
        assert_eq!(
            member.get("token").is_some(),
            status == 201,
            "only a new member is handed a key: {name}"
        );
        // A new member's answer carries its key, not its id; the roster
        // names every member by id.
        ids.push(match member["id"].as_str() {
            Some(id) => id.to_string(),
            None => listed_member_id(&client, &base, name).await,
        });
    }
    assert_eq!(ids[0], ids[2], "asking again found another loca-care");
    assert_eq!(ids[1], ids[3], "asking again found another bob");
    let desk_again = client
        .post(format!("{console}/api/members"))
        .header("x-loca-console", "1")
        .json(&serde_json::json!({ "name": "bob", "kind": "agent" }))
        .send()
        .await
        .unwrap();
    assert_eq!(desk_again.status(), 201, "the desk finds bob again");
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut names: Vec<&str> = members.iter().filter_map(|m| m["name"].as_str()).collect();
    names.sort();
    assert_eq!(names, ["bob", "loca-care"], "{members:?}");
}

/// The id the roster gives the member wearing exactly `name`.
async fn listed_member_id(client: &reqwest::Client, base: &str, name: &str) -> String {
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    members
        .iter()
        .find(|member| member["name"] == name)
        .and_then(|member| member["id"].as_str())
        .unwrap_or_else(|| panic!("{name} is not on the roster: {members:?}"))
        .to_string()
}

/// A join request is checked twice: when it is filed, against the requests
/// still waiting, and when it is approved, against the members admitted
/// since. Neither lets two names that differ only by letter case in.
#[tokio::test]
async fn a_join_request_never_admits_a_name_worn_in_another_letter_case() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[(
            "DB_PATH",
            dir.path().join("requests.db").to_string_lossy().into(),
        )],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let mint = client
        .post(format!("{base}/admission-stock"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "count": 2 }))
        .send()
        .await
        .unwrap();
    assert_eq!(mint.status(), 201, "admission stock fixture");
    let file = |name: &'static str| {
        client
            .post(format!("{base}/join-requests"))
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
    };
    let first = file("Carol").await.unwrap();
    assert_eq!(first.status(), 201);
    let first: Value = first.json().await.unwrap();
    let mut let_in = Vec::new();
    let second = file("carol").await.unwrap();
    if second.status() != 409 {
        let_in.push(format!(
            "a second request, for carol -> {}",
            second.status()
        ));
    }

    // While `Carol` waits, the Master admits `carol` directly.
    admit(&base, "MASTER", "carol", "agent").await;
    let approve = client
        .post(format!(
            "{base}/join-requests/{}/approve",
            first["request_id"].as_str().unwrap()
        ))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    if approve.status() != 409 {
        let_in.push(format!(
            "approving Carol next to carol -> {}",
            approve.status()
        ));
    }
    assert!(let_in.is_empty(), "{let_in:#?}");
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = members.iter().filter_map(|m| m["name"].as_str()).collect();
    assert_eq!(names, ["carol"], "{members:?}");
}

/// What a refused approval must leave as it was: the requests still pending,
/// the admission stock, and no membership.
async fn pending_stock_and_members(base: &str) -> (Value, Value, Vec<String>) {
    let client = reqwest::Client::new();
    let get = |path: &str| {
        client
            .get(format!("{base}{path}"))
            .header("x-admin-token", "MASTER")
            .send()
    };
    let pending: Value = get("/join-requests").await.unwrap().json().await.unwrap();
    let stock: Value = get("/admission-stock").await.unwrap().json().await.unwrap();
    let members: Vec<Value> = get("/members").await.unwrap().json().await.unwrap();
    let mut names: Vec<String> = members
        .iter()
        .filter_map(|m| m["name"].as_str().map(str::to_string))
        .collect();
    names.sort();
    (pending, stock, names)
}

/// Approving a join request admits a new name, so it meets the doors' rule
/// again. A request for `Deputy` filed before a Smaster named `deputy` was
/// created is refused at approval, at both approving doors, with 409; the
/// request stays pending, no admission right is consumed, no membership is
/// created, and the Master can still deny it.
#[tokio::test]
async fn approval_refuses_a_name_a_rank_came_to_wear_since_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let (base, console, _guard) = spawn_with_master_desk(&[(
        "DB_PATH",
        dir.path().join("approve-rank.db").to_string_lossy().into(),
    )])
    .await;
    let client = reqwest::Client::new();
    let mint = client
        .post(format!("{base}/admission-stock"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "count": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(mint.status(), 201, "admission stock fixture");
    let filed: Value = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "Deputy", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = filed["request_id"].as_str().unwrap().to_string();
    let smaster = client
        .post(format!("{base}/keys"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "kind": "smaster", "name": "deputy" }))
        .send()
        .await
        .unwrap();
    assert_eq!(smaster.status(), 201, "the Smaster takes the name first");
    let before = pending_stock_and_members(&base).await;

    let mut approved = Vec::new();
    for (door, request) in [
        (
            "POST /join-requests/:id/approve",
            client
                .post(format!("{base}/join-requests/{id}/approve"))
                .header("x-admin-token", "MASTER"),
        ),
        (
            "master desk approve",
            client
                .post(format!("{console}/api/join-requests/{id}/approve"))
                .header("x-loca-console", "1"),
        ),
    ] {
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.text().await.unwrap();
        if (status, body.as_str()) != (409, NAME_WORN_RULE) {
            approved.push(format!("{door} -> {status} {body}"));
        }
    }
    assert!(approved.is_empty(), "{approved:#?}");
    assert_eq!(
        pending_stock_and_members(&base).await,
        before,
        "a refused approval changed the request, the stock or the members"
    );
    assert!(before.2.is_empty(), "{:?}", before.2);
    let deny = client
        .post(format!("{base}/join-requests/{id}/deny"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert_eq!(deny.status(), 200, "the Master can still deny the request");
}

/// A request an older server left pending under a name no door admits now,
/// `LOCA-CARE` (the caretaker `loca-care` in another letter case) or `all`
/// (an address), is refused at approval, at both approving doors, with 409
/// and the door's sentence: no membership is created, the request stays
/// pending, the stock is untouched, and the Master can still deny it.
#[tokio::test]
async fn approval_refuses_a_name_an_older_server_filed_unchecked() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("approve-legacy.db");
    let env = [("DB_PATH", db.to_string_lossy().to_string())];
    let client = reqwest::Client::new();
    let ids = {
        let (port, guard) = spawn_server_env("MASTER", &env).await;
        let base = format!("http://127.0.0.1:{port}");
        let mint = client
            .post(format!("{base}/admission-stock"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "count": 2 }))
            .send()
            .await
            .unwrap();
        assert_eq!(mint.status(), 201, "admission stock fixture");
        let mut ids = Vec::new();
        for seed in ["care-seed", "all-seed"] {
            let filed: Value = client
                .post(format!("{base}/join-requests"))
                .json(&serde_json::json!({ "name": seed, "kind": "agent" }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            ids.push(filed["request_id"].as_str().unwrap().to_string());
        }
        drop(guard);
        wait_for_server_shutdown(port).await;
        ids
    };
    // Model the requests an older server filed without the name rule.
    let connection = rusqlite::Connection::open(&db).unwrap();
    for (seed, name) in [("care-seed", "LOCA-CARE"), ("all-seed", "all")] {
        let changed = connection
            .execute(
                "UPDATE join_requests SET name = ?2 WHERE name = ?1",
                [seed, name],
            )
            .unwrap();
        assert_eq!(changed, 1, "{seed}");
    }
    drop(connection);

    let (base, console, _guard) = spawn_with_master_desk(&env).await;
    let before = pending_stock_and_members(&base).await;
    assert!(before.2.is_empty(), "{:?}", before.2);
    let mut approved = Vec::new();
    for (id, sentence) in ids.iter().zip([NAME_WORN_RULE, ADDRESS_RULE]) {
        for (door, request) in [
            (
                "POST /join-requests/:id/approve",
                client
                    .post(format!("{base}/join-requests/{id}/approve"))
                    .header("x-admin-token", "MASTER"),
            ),
            (
                "master desk approve",
                client
                    .post(format!("{console}/api/join-requests/{id}/approve"))
                    .header("x-loca-console", "1"),
            ),
        ] {
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            let body = response.text().await.unwrap();
            if (status, body.as_str()) != (409, sentence) {
                approved.push(format!("{door} {id} -> {status} {body}"));
            }
        }
    }
    assert!(approved.is_empty(), "{approved:#?}");
    assert_eq!(
        pending_stock_and_members(&base).await,
        before,
        "a refused approval changed the requests, the stock or the members"
    );
    for id in &ids {
        let deny = client
            .post(format!("{base}/join-requests/{id}/deny"))
            .header("x-admin-token", "MASTER")
            .send()
            .await
            .unwrap();
        assert_eq!(deny.status(), 200, "the Master can still deny {id}");
    }
}

/// Guard over HTTP for the hub's race tests (`hub/tests/name_claims.rs`,
/// which are the evidence): a direct admission and a Smaster code, or two
/// direct admissions, sent together for names that fold together end with
/// exactly one of them in and the other answered 409.
#[tokio::test]
async fn admissions_sent_together_for_names_that_fold_together_let_one_in() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[(
            "DB_PATH",
            dir.path().join("together.db").to_string_lossy().into(),
        )],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let admit = |name: String| {
        client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
    };
    let smaster = |name: String| {
        client
            .post(format!("{base}/keys"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "kind": "smaster", "name": name }))
            .send()
    };
    let mut both = Vec::new();
    for round in 0..20 {
        let (left, right) =
            tokio::join!(admit(format!("Bob-{round}")), admit(format!("bob-{round}")));
        let statuses = [left.unwrap().status(), right.unwrap().status()];
        if !matches!(statuses.map(|s| s.as_u16()), [201, 409] | [409, 201]) {
            both.push(format!("members Bob-{round}/bob-{round}: {statuses:?}"));
        }
        let (left, right) =
            tokio::join!(admit(format!("Ed-{round}")), smaster(format!("ed-{round}")));
        let statuses = [left.unwrap().status(), right.unwrap().status()];
        if !matches!(statuses.map(|s| s.as_u16()), [201, 409] | [409, 201]) {
            both.push(format!(
                "member Ed-{round}, Smaster ed-{round}: {statuses:?}"
            ));
        }
    }
    assert!(both.is_empty(), "{both:#?}");
}

/// A rank cannot be created as, or renamed to, a name a member or a
/// configured caretaker wears in any letter case (a caretaker's exact name
/// included): both Smaster doors and the Master's own rename refuse it with
/// 409. Each door is asked about names of its own, so one door letting a
/// name through cannot make another door's refusal look right.
#[tokio::test]
async fn a_rank_cannot_take_a_name_worn_in_another_letter_case() {
    let dir = tempfile::tempdir().unwrap();
    let (port, _guard) = spawn_server_env(
        "MASTER",
        &[
            (
                "DB_PATH",
                dir.path().join("ranks.db").to_string_lossy().into(),
            ),
            (
                "LOCA_CARETAKERS",
                "loca-care,loca-dev,care-three,care-four".into(),
            ),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // No caretaker is admitted: its name is taken all the same.
    admit(&base, "MASTER", "bob", "agent").await;
    let mut taken = Vec::new();
    for name in ["BOB", "LOCA-CARE", "loca-care"] {
        let smaster = client
            .post(format!("{base}/smasters"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .unwrap();
        if smaster.status() != 409 {
            taken.push(format!("POST /smasters {name} -> {}", smaster.status()));
        }
    }
    for name in ["Bob", "Loca-Dev", "care-three"] {
        let key = client
            .post(format!("{base}/keys"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "kind": "smaster", "name": name }))
            .send()
            .await
            .unwrap();
        if key.status() != 409 {
            taken.push(format!("POST /keys smaster {name} -> {}", key.status()));
        }
    }
    let master = session_with(&base, ("x-admin-token", "MASTER"), "founder", None).await;
    for name in ["bOB", "CARE-FOUR"] {
        let rename = client
            .put(format!("{base}/profile"))
            .header("x-session-token", &master)
            .json(&serde_json::json!({ "display_name": name }))
            .send()
            .await
            .unwrap();
        if rename.status() != 409 {
            taken.push(format!("Master renamed to {name} -> {}", rename.status()));
        }
    }
    assert!(taken.is_empty(), "a rank took a worn name: {taken:#?}");
}

/// Guard (passes before and after the letter-case refusal): two members an
/// older server admitted under names that differ only by letter case are not
/// renamed, removed or refused. Each still signs in, is found again by its
/// exact name, is called into a loca and speaks; one `@Bob` still wakes both,
/// because mentions are matched without letter case.
#[tokio::test]
async fn two_members_differing_only_by_case_before_the_refusal_keep_working() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("legacy-case.db");
    let env = [
        ("DB_PATH", db.to_string_lossy().to_string()),
        ("REQUIRE_INVITE", "1".to_string()),
    ];
    let client = reqwest::Client::new();
    let tokens = {
        let (port, guard) = spawn_server_env("MASTER", &env).await;
        create_test_room(port, "MASTER", "parlor").await;
        let base = format!("http://127.0.0.1:{port}");
        let mut tokens = Vec::new();
        for name in ["bob", "case-seed"] {
            let admitted: Value = client
                .post(format!("{base}/members"))
                .header("x-admin-token", "MASTER")
                .json(&serde_json::json!({ "name": name, "kind": "agent" }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            tokens.push((
                admitted["token"].as_str().unwrap().to_string(),
                listed_member_id(&client, &base, name).await,
            ));
        }
        drop(guard);
        wait_for_server_shutdown(port).await;
        tokens
    };
    // Model the rows an older server wrote when it admitted `Bob` next to `bob`.
    let connection = rusqlite::Connection::open(&db).unwrap();
    connection
        .execute(
            "UPDATE members SET name = 'Bob' WHERE name = 'case-seed'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE principals SET display_name = 'Bob' WHERE display_name = 'case-seed'",
            [],
        )
        .unwrap();
    drop(connection);

    let (base, console, _guard) = spawn_with_master_desk(&env).await;
    let port: u16 = base.rsplit(':').next().unwrap().parse().unwrap();
    let mut sockets = Vec::new();
    for (name, (token, id)) in ["bob", "Bob"].into_iter().zip(&tokens) {
        let whoami: Value = client
            .get(format!("{base}/whoami"))
            .header("x-room-token", token)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(whoami["name"], name, "{whoami}");
        let again = client
            .post(format!("{base}/members"))
            .header("x-admin-token", "MASTER")
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        assert_eq!(again.status(), 200, "re-admitting {name}");
        let again: Value = again.json().await.unwrap();
        assert_eq!(&again["id"], id.as_str(), "a second {name}: {again}");
        assert!(again.get("token").is_none(), "a key was read back: {name}");
        let desk = client
            .post(format!("{console}/api/members"))
            .header("x-loca-console", "1")
            .json(&serde_json::json!({ "name": name, "kind": "agent" }))
            .send()
            .await
            .unwrap();
        assert_eq!(desk.status(), 201, "the desk refused {name}");
        let invite = invite_for(&base, "MASTER", "parlor", name).await;
        let url = format!(
            "ws://127.0.0.1:{port}/ws?room=parlor&name={name}&type=agent&filter=mentions&turn_max=1&token={invite}"
        );
        let (ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
        wait_for_member(
            &client,
            &base,
            "parlor",
            name,
            Some(("x-admin-token", "MASTER")),
        )
        .await;
        sockets.push(ws);
        let session = session_with(&base, ("x-room-token", &invite), name, None).await;
        let posted = client
            .post(format!("{base}/rooms/parlor/messages"))
            .header("x-session-token", &session)
            .json(&serde_json::json!({ "sender": name, "sender_type": "agent", "text": format!("{name} here") }))
            .send()
            .await
            .unwrap();
        assert!(posted.status().is_success(), "{name}: {}", posted.status());
    }
    let members: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mut names: Vec<&str> = members.iter().filter_map(|m| m["name"].as_str()).collect();
    names.sort();
    assert_eq!(names, ["Bob", "bob"], "{members:?}");

    let carol = invite_for(&base, "MASTER", "parlor", "carol").await;
    let carol = session_with(&base, ("x-room-token", &carol), "carol", None).await;
    client
        .post(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", &carol)
        .json(&serde_json::json!({ "sender": "carol", "sender_type": "agent", "text": "@Bob look at this" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
    for ws in &mut sockets {
        wait_for(ws, |v| {
            v["t"] == "msg" && v["message"]["text"] == "@Bob look at this"
        })
        .await;
    }
}

/// Revoking a MEMBERSHIP cascades to every invite and session: losing the
/// building loses every seat and every proof at once.
#[tokio::test]
async fn revoking_membership_kills_invites_and_sessions() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    create_test_room(port, "MASTER", "parlor1").await;
    create_test_room(port, "MASTER", "parlor2").await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    // cyber is a member seated in two locas.
    admit(&base, "MASTER", "cyber", "agent").await;
    let mb: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mbtok = mb.iter().find(|m| m["name"] == "cyber").unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        mb.iter().all(|member| member["token"].is_null()),
        "membership list must redact bearer secrets"
    );
    let d1 = invite_for(&base, "MASTER", "parlor1", "cyber").await;
    let d2 = invite_for(&base, "MASTER", "parlor2", "cyber").await;
    let st = session_with(&base, ("x-room-token", &d1), "cyber", None).await;
    assert_eq!(
        client
            .get(format!("{base}/rooms/parlor1/messages"))
            .header("x-session-token", &st)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    // Revoke the membership.
    client
        .delete(format!("{base}/members/{mbtok}"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    // Both invites are dead, and the session is dead.
    for d in [&d1, &d2] {
        assert_eq!(
            client
                .get(format!("{base}/rooms/parlor1/messages"))
                .header("x-room-token", d)
                .send()
                .await
                .unwrap()
                .status(),
            401,
            "a revoked member's invites no longer open anything"
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/rooms/parlor1/messages"))
            .header("x-session-token", &st)
            .send()
            .await
            .unwrap()
            .status(),
        401,
        "and the session dies with the membership"
    );
}

/// A member can ask "who am I" with their mb_ token and be recognized.
#[tokio::test]
async fn whoami_recognizes_a_member() {
    // Invite-only is the production shape. The permanent mb_ token must pass
    // the blanket API gate even though it deliberately opens no loca.
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let m: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let mbtok = m["token"].as_str().unwrap();
    let who: Value = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", mbtok)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(who["kind"], "member");
    assert_eq!(who["name"], "cyber");
}

/// Admissions are a building-admin act, not master-only: a Smaster can list and
/// act on join requests and mint admission stock, exactly as they can admit
/// members. The approve route already records `smaster:{name}`, so the auth
/// check must accept a Smaster (was wrongly `is_master_req`, now `is_admin_req`).
#[tokio::test]
async fn smaster_can_manage_join_requests_and_stock() {
    // Join requests live in the store (not the in-memory member cache), so this
    // needs a real connection — the default no-DB_PATH server is a no-op store.
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", ":memory:".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // The Master appoints a Smaster.
    let sm: Value = client
        .post(format!("{base}/smasters"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "deputy" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let sm_token = sm["token"].as_str().unwrap();

    // The Smaster can read the pending list — 401 back when it was master-only.
    let list = client
        .get(format!("{base}/join-requests"))
        .header("x-admin-token", sm_token)
        .send()
        .await
        .unwrap();
    assert_eq!(
        list.status(),
        200,
        "a smaster must be able to list join requests"
    );

    // …and mint admission stock.
    let mint = client
        .post(format!("{base}/admission-stock"))
        .header("x-admin-token", sm_token)
        .json(&serde_json::json!({ "count": 1 }))
        .send()
        .await
        .unwrap();
    assert_eq!(
        mint.status(),
        201,
        "a smaster must be able to mint admission stock"
    );

    // The Smaster can APPROVE: an outside agent requests, the smaster approves,
    // and the single minted right is consumed EXACTLY once.
    let created: Value = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "guest", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jr_id = created["request_id"].as_str().unwrap();
    let approve = client
        .post(format!("{base}/join-requests/{jr_id}/approve"))
        .header("x-admin-token", sm_token)
        .send()
        .await
        .unwrap();
    assert_eq!(approve.status(), 200, "a smaster must be able to approve");
    let stock: Value = client
        .get(format!("{base}/admission-stock"))
        .header("x-admin-token", sm_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stock["available"], 0,
        "approve consumes the one minted right exactly once"
    );

    // …and DENY a second request.
    let created2: Value = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "guest2", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jr2 = created2["request_id"].as_str().unwrap();
    let deny = client
        .post(format!("{base}/join-requests/{jr2}/deny"))
        .header("x-admin-token", sm_token)
        .send()
        .await
        .unwrap();
    assert_eq!(deny.status(), 200, "a smaster must be able to deny");

    // Neither the approved nor the denied request remains pending.
    let pending: Value = client
        .get(format!("{base}/join-requests"))
        .header("x-admin-token", sm_token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = pending["pending"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(
        !names.contains(&"guest") && !names.contains(&"guest2"),
        "approved and denied requests leave the pending list"
    );

    // A non-admin (no credential) is still refused at the handler.
    let anon = client
        .get(format!("{base}/join-requests"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        anon.status(),
        401,
        "a non-admin must not read join requests"
    );
}

#[tokio::test]
async fn join_request_bootstrap_is_refetchable_until_ack_over_http() {
    // The crash-safe onboarding contract at the HTTP boundary: an outside agent
    // requests, a Master approves, the mb_ is RE-FETCHABLE with the request
    // secret (so a crash between receiving it and persisting it loses nothing),
    // authenticates via /whoami, then ONE /ack closes the window — after which
    // bootstrap is 404 while the delivered membership keeps working. Needs a real
    // store (the default no-DB server is a no-op store for join requests).
    let (port, _guard) = spawn_server_env("MASTER", &[("DB_PATH", ":memory:".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Stock + request + approve.
    client
        .post(format!("{base}/admission-stock"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "count": 1 }))
        .send()
        .await
        .unwrap();
    let created: Value = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "newbie", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jr = created["request_id"].as_str().unwrap().to_string();
    let secret = created["request_secret"].as_str().unwrap().to_string();
    assert!(
        secret.starts_with("jrs_"),
        "the request secret is the requester's only handle"
    );
    client
        .post(format!("{base}/join-requests/{jr}/approve"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();

    // Poll now advertises bootstrap_ready.
    let poll: Value = client
        .get(format!("{base}/join-requests/{jr}"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(poll["status"], "approved");
    assert_eq!(poll["bootstrap_ready"], true);

    // Two bootstraps BEFORE ack return the SAME mb_ (crash-safe re-fetch).
    let r1 = client
        .post(format!("{base}/join-requests/{jr}/bootstrap"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(r1.status(), 201);
    let mb1 = r1.json::<Value>().await.unwrap()["davet"]
        .as_str()
        .unwrap()
        .to_string();
    let r2 = client
        .post(format!("{base}/join-requests/{jr}/bootstrap"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(r2.status(), 201, "bootstrap is re-fetchable until ack");
    let mb2 = r2.json::<Value>().await.unwrap()["davet"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(mb1, mb2, "the credential never diverges across re-fetches");
    assert!(mb1.starts_with("mb_"));

    // The mb_ authenticates as the named member via /whoami.
    let who: Value = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", &mb1)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(who["kind"], "member");
    assert_eq!(who["name"], "newbie");

    // A wrong secret can neither bootstrap nor ack, and leaves the window OPEN.
    let bad_boot = client
        .post(format!("{base}/join-requests/{jr}/bootstrap"))
        .header("x-join-secret", "jrs_wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_boot.status(), 404);
    let bad_ack = client
        .post(format!("{base}/join-requests/{jr}/ack"))
        .header("x-join-secret", "jrs_wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(bad_ack.status(), 404);
    let still = client
        .post(format!("{base}/join-requests/{jr}/bootstrap"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(
        still.status(),
        201,
        "a wrong-secret ack must not close the window"
    );

    // ACK closes the window; the response carries NO credential/secret.
    let ack = client
        .post(format!("{base}/join-requests/{jr}/ack"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(ack.status(), 200);
    let ack_body: Value = ack.json().await.unwrap();
    assert_eq!(ack_body["acknowledged"], true);
    assert!(
        ack_body.get("davet").is_none() && ack_body.get("mb").is_none(),
        "the ack response must not echo any credential"
    );
    // Repeat ack is idempotent 200.
    let ack2 = client
        .post(format!("{base}/join-requests/{jr}/ack"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(ack2.status(), 200, "repeat ack is idempotent");

    // After ACK the bootstrap window is CLOSED (404)…
    let after = client
        .post(format!("{base}/join-requests/{jr}/bootstrap"))
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap();
    assert_eq!(after.status(), 404, "bootstrap is closed after ack");
    // …but the delivered membership itself still works (only the one-time copy
    // in the request row was scrubbed).
    let who2 = client
        .get(format!("{base}/whoami"))
        .header("x-room-token", &mb1)
        .send()
        .await
        .unwrap();
    assert_eq!(
        who2.status(),
        200,
        "the delivered membership stays valid after ack"
    );
}

/// A database that cannot be read for a moment is not a request that does
/// not exist. The skill's helper forgets its request (and would file a new
/// one) on a 404 from the poll, and gives up for good on a 404 from the
/// collection, so a storage error on any of the three requester endpoints
/// must be a retryable 503. Wrong secret and unknown id stay 404, and once
/// the database answers again the same request carries on.
#[tokio::test]
async fn a_storage_error_on_a_join_request_is_a_retryable_503_not_a_404() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("join-503.sqlite");
    let (port, _guard) =
        spawn_server_env("MASTER", &[("DB_PATH", db.to_string_lossy().to_string())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    client
        .post(format!("{base}/admission-stock"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "count": 1 }))
        .send()
        .await
        .unwrap();
    let created: Value = client
        .post(format!("{base}/join-requests"))
        .json(&serde_json::json!({ "name": "unlucky", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let jr = created["request_id"].as_str().unwrap().to_string();
    let secret = created["request_secret"].as_str().unwrap().to_string();
    let approved = client
        .post(format!("{base}/join-requests/{jr}/approve"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap();
    assert_eq!(approved.status(), 200);

    let ask = |method: reqwest::Method, path: String, secret: String| {
        let client = client.clone();
        async move {
            client
                .request(method, path)
                .header("x-join-secret", secret)
                .send()
                .await
                .unwrap()
                .status()
                .as_u16()
        }
    };
    let poll = format!("{base}/join-requests/{jr}");
    let collect = format!("{base}/join-requests/{jr}/bootstrap");
    let ack = format!("{base}/join-requests/{jr}/ack");

    // The table is out of reach: every read the requester's endpoints make fails.
    let side = rusqlite::Connection::open(&db).unwrap();
    side.execute_batch("ALTER TABLE join_requests RENAME TO join_requests_away")
        .unwrap();
    assert_eq!(
        ask(reqwest::Method::GET, poll.clone(), secret.clone()).await,
        503
    );
    assert_eq!(
        ask(reqwest::Method::POST, collect.clone(), secret.clone()).await,
        503
    );
    assert_eq!(
        ask(reqwest::Method::POST, ack.clone(), secret.clone()).await,
        503
    );

    // Back: a wrong secret and an unknown id are still a plain 404, and the
    // request itself was never lost.
    side.execute_batch("ALTER TABLE join_requests_away RENAME TO join_requests")
        .unwrap();
    assert_eq!(
        ask(reqwest::Method::GET, poll.clone(), "jrs_wrong".into()).await,
        404
    );
    assert_eq!(
        ask(reqwest::Method::POST, collect.clone(), "jrs_wrong".into()).await,
        404
    );
    assert_eq!(
        ask(reqwest::Method::POST, ack.clone(), "jrs_wrong".into()).await,
        404
    );
    assert_eq!(
        ask(
            reqwest::Method::GET,
            format!("{base}/join-requests/jr_unknown"),
            secret.clone()
        )
        .await,
        404
    );
    let view: Value = client
        .get(&poll)
        .header("x-join-secret", &secret)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(view["status"], "approved");
    assert_eq!(view["bootstrap_ready"], true);
    assert_eq!(view["bootstrap_expired"], false);
    assert_eq!(
        ask(reqwest::Method::POST, collect, secret.clone()).await,
        201
    );
    assert_eq!(ask(reqwest::Method::POST, ack, secret).await, 200);
}

/// Appearance preferences are personal: each principal reads and writes only
/// their own row, values are a closed set, and the row survives a restart.
#[tokio::test]
async fn profile_preferences_are_private_validated_and_durable() {
    let root = "root-preferences-test";
    let db_dir = tempfile::tempdir().unwrap();
    let db_path = db_dir.path().join("preferences.sqlite3");
    let env = [("DB_PATH", db_path.to_string_lossy().into_owned())];
    let (port, guard) = spawn_server_env(root, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    let mut keys = Vec::new();
    for name in ["pref-ada", "pref-bo"] {
        let member: Value = client
            .post(format!("{base}/members"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "name": name, "kind": "user" }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        keys.push(member["token"].as_str().unwrap().to_string());
    }
    let ada = session_with(&base, ("x-admin-token", &keys[0]), "pref-ada", None).await;
    let bo = session_with(&base, ("x-admin-token", &keys[1]), "pref-bo", None).await;
    let url = format!("{base}/profile/preferences");

    // Signed-out callers get nothing, not even the defaults.
    let anonymous = client.get(&url).send().await.unwrap();
    assert_eq!(anonymous.status(), reqwest::StatusCode::UNAUTHORIZED);
    let anonymous_put = client
        .put(&url)
        .json(&serde_json::json!({ "theme": "light" }))
        .send()
        .await
        .unwrap();
    assert_eq!(anonymous_put.status(), reqwest::StatusCode::UNAUTHORIZED);

    let defaults: Value = client
        .get(&url)
        .header("x-session-token", &ada)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        defaults,
        serde_json::json!({
            "theme": "loca", "accent": "teal", "density": "comfortable", "font_size": "normal",
            "font_family": "sans", "accent_custom": "", "layout": "bubbles", "time_format": "auto", "measure": "normal", "own_tint": "on", "motion": "system",
            "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
            "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
        })
    );

    let saved: Value = client
        .put(&url)
        .header("x-session-token", &ada)
        .json(&serde_json::json!({ "theme": "light", "accent": "violet", "density": "compact" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let expected = serde_json::json!({
        "theme": "light", "accent": "violet", "density": "compact", "font_size": "normal",
        "font_family": "sans", "accent_custom": "", "layout": "bubbles", "time_format": "auto", "measure": "normal", "own_tint": "on", "motion": "system",
        "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
        "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
    });
    assert_eq!(saved, expected);

    // A partial update keeps the other fields.
    let partial: Value = client
        .put(&url)
        .header("x-session-token", &ada)
        .json(&serde_json::json!({ "font_size": "large" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(partial["accent"], "violet");
    assert_eq!(partial["font_size"], "large");

    // The added choices: each new value is accepted over the wire.
    for (key, value) in [
        ("density", "spacious"),
        ("font_size", "small"),
        ("font_size", "larger"),
        ("font_family", "sans"),
        ("font_family", "serif"),
    ] {
        let saved: Value = client
            .put(&url)
            .header("x-session-token", &ada)
            .json(&serde_json::json!({ key: value }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(saved[key], value);
    }
    // An older client sends only the four keys it knows: accepted, and the
    // font it never heard of keeps its value.
    let old_client: Value = client
        .put(&url)
        .header("x-session-token", &ada)
        .json(&serde_json::json!({
            "theme": "light", "accent": "violet", "density": "compact", "font_size": "large"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(old_client["font_family"], "serif");

    // Every value outside the allowed set is refused, and nothing changes.
    for bad in [
        serde_json::json!({ "theme": "solarized" }),
        serde_json::json!({ "accent": "#ff0000" }),
        serde_json::json!({ "density": "tiny" }),
        serde_json::json!({ "font_size": 18 }),
        serde_json::json!({ "font_size": "huge" }),
        serde_json::json!({ "density": "spacious", "font_family": "comic-sans" }),
        serde_json::json!({ "font_family": "Georgia, serif" }),
        serde_json::json!({ "theme": "light", "owner": "pref-bo" }),
        serde_json::json!({ "sound_volume": "max" }),
        serde_json::json!({ "sound_dm": "off", "sound": "loud" }),
        serde_json::json!(["light"]),
    ] {
        let response = client
            .put(&url)
            .header("x-session-token", &ada)
            .json(&bad)
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "{bad} must be refused"
        );
    }
    let not_json = client
        .put(&url)
        .header("x-session-token", &ada)
        .body("theme=light")
        .send()
        .await
        .unwrap();
    assert_eq!(not_json.status(), reqwest::StatusCode::BAD_REQUEST);

    // Another person's session sees only their own (default) preferences.
    let bo_view: Value = client
        .get(&url)
        .header("x-session-token", &bo)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bo_view, defaults, "bo never sees ada's preferences");
    let bo_saved: Value = client
        .put(&url)
        .header("x-session-token", &bo)
        .json(&serde_json::json!({ "accent": "amber" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bo_saved["theme"], "loca");
    assert_eq!(bo_saved["accent"], "amber");

    let expected = serde_json::json!({
        "theme": "light", "accent": "violet", "density": "compact", "font_size": "large",
        "font_family": "serif", "accent_custom": "", "layout": "bubbles", "time_format": "auto", "measure": "normal", "own_tint": "on", "motion": "system",
        "sound": "on", "sound_message": "off", "sound_mention": "on", "sound_dm": "on",
        "sound_notice": "on", "sound_volume": "medium", "sound_background": "on"
    });
    let ada_after: Value = client
        .get(&url)
        .header("x-session-token", &ada)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ada_after, expected, "bo's write never touches ada's row");

    drop(guard);
    wait_for_server_shutdown(port).await;
    let (port, _guard) = spawn_server_env(root, &env).await;
    let base = format!("http://127.0.0.1:{port}");
    let url = format!("{base}/profile/preferences");
    let restored: Value = client
        .get(&url)
        .header("x-admin-token", &keys[0])
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(restored, expected, "preferences survive a restart");
    let fresh_bo = session_with(&base, ("x-admin-token", &keys[1]), "pref-bo", None).await;
    let bo_restored: Value = client
        .get(&url)
        .header("x-session-token", &fresh_bo)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(bo_restored["accent"], "amber");
    assert_eq!(bo_restored["theme"], "loca");
}

/// Read an open socket until it ends. It must end, and it must not deliver
/// `text` (posted after the revocation) before it does.
async fn ends_without(
    ws: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    text: &str,
    what: &str,
) {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) | Some(Ok(WsMessage::Close(_))) => return,
                Some(Ok(WsMessage::Text(frame))) => assert!(
                    !frame.contains(text),
                    "{what}: a revoked connection received a frame posted after the revocation"
                ),
                Some(Ok(_)) => {}
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}: a revoked connection stays open"));
}

/// Both doors that take a Smaster code back, the Master's management
/// endpoint (by `smid_`) and the Smaster's own profile (revoking the legacy
/// credential), end it everywhere: an open WebSocket held by the code or by
/// a session taken with it, that session, a reconnect by subprotocol or by the
/// legacy query, the Building registry, and a restart.
#[tokio::test]
async fn both_smaster_revocation_doors_end_the_code_everywhere() {
    for door in ["management", "profile"] {
        let root = "smaster-doors-root";
        let dir = tempfile::tempdir().unwrap();
        let env = [
            (
                "DB_PATH",
                dir.path().join("doors.db").to_string_lossy().into_owned(),
            ),
            ("REQUIRE_INVITE", "1".to_string()),
            ("REQUIRE_SESSIONS", "1".to_string()),
        ];
        let (port, guard) = spawn_server_env(root, &env).await;
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::Client::new();
        // One loca per socket: in one loca the two would share the
        // Smaster's seat and the second would evict the first.
        create_test_room(port, root, "doors").await;
        create_test_room(port, root, "doors-session").await;
        let name = format!("deputy-{door}");
        let minted: Value = client
            .post(format!("{base}/smasters"))
            .header("x-admin-token", root)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .await
            .unwrap();
        let code = minted["token"].as_str().unwrap().to_string();
        let session = session_with(&base, ("x-admin-token", &code), &name, None).await;
        let url = format!("ws://127.0.0.1:{port}/ws?room=doors&name={name}&type=user");
        let mut by_code = connect_ws_protocols(
            url.clone(),
            &["loca.v1".into(), format!("loca.admin.{code}")],
        )
        .await;
        let mut by_session = connect_ws_protocols(
            url.replace("room=doors&", "room=doors-session&"),
            &["loca.v1".into(), format!("loca.session.{session}")],
        )
        .await;
        let master = session_with(&base, ("x-admin-token", root), "Master", None).await;
        let post = |room: &'static str, text: String| {
            let client = client.clone();
            let base = base.clone();
            let master = master.clone();
            async move {
                client
                    .post(format!("{base}/rooms/{room}/messages"))
                    .header("x-session-token", &master)
                    .json(&serde_json::json!({ "sender": "Master", "sender_type": "user", "text": text }))
                    .send()
                    .await
                    .unwrap()
                    .error_for_status()
                    .unwrap();
            }
        };
        // Both sockets are live before the revocation.
        for (room, ws) in [("doors", &mut by_code), ("doors-session", &mut by_session)] {
            wait_for_member(&client, &base, room, &name, Some(("x-admin-token", root))).await;
            let before = format!("before the {door} revocation in {room}");
            post(room, before.clone()).await;
            wait_for(ws, |frame| frame.to_string().contains(&before)).await;
        }

        match door {
            "management" => {
                let listed: Vec<Value> = client
                    .get(format!("{base}/smasters"))
                    .header("x-admin-token", root)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                let id = listed.iter().find(|s| s["name"] == name.as_str()).unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let revoked = client
                    .delete(format!("{base}/smasters/{id}"))
                    .header("x-admin-token", root)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(revoked.status().as_u16(), 200);
            }
            _ => {
                let credentials: Vec<Value> = client
                    .get(format!("{base}/profile/credentials"))
                    .header("x-admin-token", &code)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                let legacy = credentials
                    .iter()
                    .find(|c| c["label"] == "Smaster access")
                    .unwrap()["id"]
                    .as_str()
                    .unwrap()
                    .to_string();
                let revoked = client
                    .delete(format!("{base}/profile/credentials/{legacy}"))
                    .header("x-admin-token", &code)
                    .send()
                    .await
                    .unwrap();
                assert_eq!(revoked.status().as_u16(), 204);
            }
        }

        // Open sockets: the next frame finds the authority gone.
        let after = format!("posted after the {door} revocation");
        post("doors", after.clone()).await;
        post("doors-session", after.clone()).await;
        ends_without(&mut by_code, &after, door).await;
        ends_without(&mut by_session, &after, door).await;

        // The Building registry no longer lists it, and nothing reopens it.
        let check = |base: String, port: u16| {
            let client = client.clone();
            let code = code.clone();
            let session = session.clone();
            let url = format!("ws://127.0.0.1:{port}/ws?room=doors&name={name}&type=user");
            let name = name.clone();
            async move {
                let listed: Vec<Value> = client
                    .get(format!("{base}/smasters"))
                    .header("x-admin-token", root)
                    .send()
                    .await
                    .unwrap()
                    .json()
                    .await
                    .unwrap();
                assert!(
                    listed.iter().all(|s| s["name"] != name.as_str()),
                    "{door}: the registry still lists the revoked Smaster"
                );
                for (header, value) in [("x-admin-token", &code), ("x-session-token", &session)] {
                    for path in ["/profile", "/building/ranks", "/rooms"] {
                        let status = client
                            .get(format!("{base}{path}"))
                            .header(header, value.as_str())
                            .send()
                            .await
                            .unwrap()
                            .status()
                            .as_u16();
                        assert!(
                            status >= 400,
                            "{door}: {header} still reaches {path} ({status})"
                        );
                    }
                }
                let retake = client
                    .post(format!("{base}/sessions"))
                    .header("x-admin-token", code.as_str())
                    .json(&serde_json::json!({ "name": "again", "kind": "user" }))
                    .send()
                    .await
                    .unwrap();
                assert!(retake.status().is_client_error(), "{door}: a new session");
                for protocol in [
                    format!("loca.admin.{code}"),
                    format!("loca.session.{session}"),
                ] {
                    let mut request = url.clone().into_client_request().unwrap();
                    request.headers_mut().insert(
                        "Sec-WebSocket-Protocol",
                        HeaderValue::from_str(&format!("loca.v1, {protocol}")).unwrap(),
                    );
                    assert!(
                        tokio_tungstenite::connect_async(request).await.is_err(),
                        "{door}: a WebSocket reconnect"
                    );
                }
                // The legacy query door (enabled for these tests only) asks
                // the same question.
                assert!(!ws_can(port, "doors", &format!("admin={code}")).await);
                assert!(!ws_can(port, "doors-session", &format!("session={session}")).await);
            }
        };
        check(base.clone(), port).await;
        drop(guard);
        wait_for_server_shutdown(port).await;
        let (restart_port, _restart) = spawn_server_env(root, &env).await;
        check(format!("http://127.0.0.1:{restart_port}"), restart_port).await;
    }
}
