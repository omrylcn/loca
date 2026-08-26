//! Building membership, davets, residents, and identity seats.

use super::*;

/// Reading is as private as writing: with a room token set, an outsider must
/// not be able to read history, notes, tasks or the roster. (Regression for a
/// live leak: writes were gated, reads were not.)
#[tokio::test]
async fn reads_require_membership_too() {
    let (port, _guard) = spawn_server_env("adm", &[("ROOM_TOKEN", "join".into())]).await;
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

/// Membership and a davet are different acts, and the difference is the point.
///
/// Belonging to the building creates an identity: heavy, rare, done from a
/// terminal. A davet seats an existing member in one loca: light, frequent,
/// done from the UI. Leaving a loca must never cost the building — otherwise
/// every call-in starts from scratch again, which is the whole problem.
#[tokio::test]
async fn membership_outlives_the_locas_you_are_called_into() {
    let (port, _guard) = spawn_server_env("MASTER", &[("ROOM_TOKEN", "building".into())]).await;
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
        "calling a member in gives them a davet"
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

    // Take the seat away: the davet ends, the BELONGING does not. They stay a
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
        "list must redact the davet secret"
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
        .expect("membership outlives the davet — still a resident");
    assert!(
        me["locas"].as_array().unwrap().is_empty(),
        "the seat is gone once the davet is revoked"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Membership is the constitution: a davet seats an EXISTING member, it never
// creates identity. "Knowing someone and inviting them to the table are not
// the same act."
// ─────────────────────────────────────────────────────────────────────────────

/// A davet for an unknown name is refused — admitting is its own act.
#[tokio::test]
async fn invite_requires_existing_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let r = reqwest::Client::new()
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "stranger" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 409, "a davet never creates identity");
}

/// Calling in an unknown name is refused with directions, not quietly obeyed.
#[tokio::test]
async fn call_requires_existing_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
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

/// Issuing a davet must not grow the membership list as a side effect.
#[tokio::test]
async fn invite_does_not_create_identity() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
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
    let _d = davet_for(&base, "MASTER", "general", "cyber").await;
    let after: Vec<Value> = client
        .get(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before.len(), after.len(), "a davet is not a membership");
}

/// Admit & invite are two records, and the davet points at the membership.
#[tokio::test]
async fn admit_and_invite_creates_two_distinct_records() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
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
        inv["member"].as_str().unwrap(),
        mb,
        "the davet names its member"
    );
}

/// A session taken with alice's davet IS alice — whatever the body claims.
#[tokio::test]
async fn session_identity_comes_from_invited_member() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = davet_for(&base, "MASTER", "general", "alice").await;
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
        "the davet says who you are, not the body"
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
    let base = format!("http://127.0.0.1:{port}");
    admit(&base, "MASTER", "known", "agent").await;
    // A stranger connects with the building key — seated, but not a resident.
    let url =
        format!("ws://127.0.0.1:{port}/ws?room=general&name=stranger&type=agent&token=building");
    let (_ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
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

/// A member with no davet anywhere is in the lobby: visible and callable, but
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

/// One member, one loca, one live davet — the second ask says so.
#[tokio::test]
async fn duplicate_invite_for_same_member_and_room_is_rejected() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let _first = davet_for(&base, "MASTER", "general", "cyber").await;
    let again = reqwest::Client::new()
        .post(format!("{base}/rooms/general/invites"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber" }))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 409, "one live davet per member per loca");
}

/// Old databases carry davets with no member link. Boot binds each to the
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
    // The old davet still opens its loca — nobody is locked out by the upgrade.
    let r = client
        .get(format!("{base}/rooms/general/messages"))
        .header("x-room-token", "dv_legacy_cyber")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "a legacy davet still works");
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
        "the davet is bound to the redacted membership id"
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

/// Same davet, spoofed query name → one seat under the invited identity.
#[tokio::test]
async fn same_davet_two_names_one_seat() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let d = davet_for(&base, "MASTER", "general", "cyber").await;
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
    assert_eq!(members.len(), 1, "one davet, one seat");
    assert_eq!(
        members[0]["name"], "cyber",
        "a davet's recorded identity overrides query-string aliases"
    );
}

/// Two DIFFERENT members picking the same display name are still two people.
#[tokio::test]
async fn two_different_members_same_display_name_two_seats() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let d1 = davet_for(&base, "MASTER", "general", "g1").await;
    let d2 = davet_for(&base, "MASTER", "general", "g2").await;
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
    let base = format!("http://127.0.0.1:{port}");
    let (mut a, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=oda-a&name=master&type=user&admin=MASTER"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut a, |v| v["t"] == "history").await;
    let (mut b, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=oda-b&name=master&type=user&admin=MASTER"
    ))
    .await
    .unwrap();
    let _ = wait_for(&mut b, |v| v["t"] == "history").await;
    for room in ["oda-a", "oda-b"] {
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
    let base = format!("http://127.0.0.1:{port}");
    // Seven members take the seven seats.
    let mut held = Vec::new();
    for i in 1..=7 {
        let d = davet_for(&base, "MASTER", "dolu", &format!("m{i}")).await;
        let (mut ws, _) = tokio_tungstenite::connect_async(format!(
            "ws://127.0.0.1:{port}/ws?room=dolu&name=m{i}&type=agent&token={d}"
        ))
        .await
        .unwrap();
        let _ = wait_for(&mut ws, |v| v["t"] == "history").await;
        held.push((ws, d));
    }
    // A seated identity re-enters under a new name: no overflow, same seat.
    let d1 = held[0].1.clone();
    let (mut renamed, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/ws?room=dolu&name=m1-yeni&type=agent&token={d1}"
    ))
    .await
    .unwrap();
    let frame = wait_for(&mut renamed, |v| v["t"] == "history" || v["t"] == "control").await;
    assert_eq!(
        frame["t"], "history",
        "your own seat is always yours to retake"
    );
    let members: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/rooms/dolu/members"))
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
    // A genuinely new identity is turned away at mint (Model A: seven davets,
    // no eighth). The rename above did not consume an extra seat, so the seven
    // davets are still all that exist and the eighth mint is refused.
    admit(&base, "MASTER", "m8", "agent").await;
    let eighth = reqwest::Client::new()
        .post(format!("{base}/rooms/dolu/invites"))
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

/// DELETE /rooms/A/invites/<tokenB> must not revoke loca B's davet through
/// loca A's door — the URL's loca and the token's loca must agree.
#[tokio::test]
async fn revoke_invite_checks_the_loca() {
    let (port, _g) = spawn_server_env("MASTER", &[]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = davet_for(&base, "MASTER", "real", "cyber").await;
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
    // The davet still works.
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

/// An agent can say "işim bitti" for its own seat. No operator name is trusted
/// from a body: the davet at the door identifies exactly which membership and
/// loca may be released. The davet dies; membership returns to the lobby.
#[tokio::test]
async fn a_member_can_release_its_own_loca_into_the_lobby() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let davet = davet_for(&base, "MASTER", "oda", "worker").await;

    let released = client
        .post(format!("{base}/rooms/oda/release"))
        .header("x-room-token", &davet)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);

    let old_door = client
        .get(format!("{base}/rooms/oda/messages"))
        .header("x-room-token", &davet)
        .send()
        .await
        .unwrap();
    assert_eq!(old_door.status(), 401, "self-release ends the loca davet");

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
        .post(format!("{base}/rooms/oda/call"))
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

/// Kick stops the davet but keeps the membership: the master can call them
/// straight back in (a fresh davet), because kick is not a building expulsion.
#[tokio::test]
async fn kick_keeps_membership_so_recall_is_one_step() {
    // Davet-only mode: the davet is the ONLY door, so stopping it truly shuts
    // the loca (in open mode the door is davet-independent and this can't show).
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    let d = davet_for(&base, "MASTER", "oda", "cyber").await;
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    client
        .post(format!("{base}/rooms/oda/moderate"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "action": "kick", "name": "cyber" }))
        .send()
        .await
        .unwrap();
    // Old davet is dead.
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-room-token", &d)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // But they are still a building member — call them back in one step.
    let again = client
        .post(format!("{base}/rooms/oda/call"))
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
            .get(format!("{base}/rooms/oda/messages"))
            .header("x-room-token", fresh["token"].as_str().unwrap())
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "the fresh davet opens the loca again"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// FAZ 2 — kimlik: üye tekilliği, kind, revoke cascade, whoami.
// PRINCIPLES: üyelik=kimlik, davet=koltuk; parent ölünce child ölür.
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
    let second: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", "MASTER")
        .json(&serde_json::json!({ "name": "cyber", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        first["token"], second["token"],
        "same name = same membership token"
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

/// Revoking a MEMBERSHIP cascades to every davet and session: losing the
/// building loses every seat and every proof at once.
#[tokio::test]
async fn revoking_membership_kills_davets_and_sessions() {
    let (port, _g) = spawn_server_env("MASTER", &[("REQUIRE_INVITE", "1".into())]).await;
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
    let d1 = davet_for(&base, "MASTER", "oda1", "cyber").await;
    let d2 = davet_for(&base, "MASTER", "oda2", "cyber").await;
    let st = session_with(&base, ("x-room-token", &d1), "cyber", None).await;
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda1/messages"))
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
    // Both davets are dead, and the session is dead.
    for d in [&d1, &d2] {
        assert_eq!(
            client
                .get(format!("{base}/rooms/oda1/messages"))
                .header("x-room-token", d)
                .send()
                .await
                .unwrap()
                .status(),
            401,
            "a revoked member's davets no longer open anything"
        );
    }
    assert_eq!(
        client
            .get(format!("{base}/rooms/oda1/messages"))
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
    // Davet-only is the production shape. The permanent mb_ token must pass
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
