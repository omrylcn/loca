//! Claiming a membership key with an invite, over the real server: one key per
//! set-up, retries that return the same key, the ACK, the cap of five, the
//! request an older kit makes, the two revoke doors, and the answers that no
//! longer hand out a member's own key.
use super::*;

const ROOT: &str = "claim-fixture-root";

/// Claim ids shaped like the kit's (`cl_` and 43 URL-safe characters).
const SETUP_A: &str = "cl_FixtureSetupAaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SETUP_B: &str = "cl_FixtureSetupBbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const SETUP_C: &str = "cl_FixtureSetupCccccccccccccccccccccccccccccc";
const SETUP_D: &str = "cl_FixtureSetupDddddddddddddddddddddddddddddd";
const SETUP_E: &str = "cl_FixtureSetupEeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
const SETUP_F: &str = "cl_FixtureSetupFfffffffffffffffffffffffffffff";

const CLAIMED_LABEL: &str = "Membership key (claimed with davet for #parlor)";

fn persistent(db: &std::path::Path) -> Vec<(&'static str, String)> {
    vec![
        ("DB_PATH", db.to_string_lossy().into_owned()),
        ("REQUIRE_INVITE", "1".into()),
    ]
}

/// A Building with `worker` admitted and seated in `parlor`: the member's
/// original key (handed out once, at admission) and its invite.
async fn seated(base: &str) -> (String, String) {
    let client = reqwest::Client::new();
    let created = client
        .post(format!("{base}/rooms"))
        .header("x-admin-token", ROOT)
        .json(&serde_json::json!({ "name": "parlor" }))
        .send()
        .await
        .unwrap();
    assert_eq!(created.status(), 201);
    let member: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", ROOT)
        .json(&serde_json::json!({ "name": "worker", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let invite: Value = client
        .post(format!("{base}/rooms/parlor/invites"))
        .header("x-admin-token", ROOT)
        .json(&serde_json::json!({ "name": "worker" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    (
        member["token"].as_str().unwrap().to_string(),
        invite["token"].as_str().unwrap().to_string(),
    )
}

/// `POST /membership/claim` as the new kit sends it.
async fn claim(base: &str, invite: &str, setup: Option<&str>) -> (u16, Value, String) {
    let mut request = reqwest::Client::new()
        .post(format!("{base}/membership/claim"))
        .header("x-room-token", invite);
    if let Some(setup) = setup {
        request = request.header("x-claim-id", setup);
    }
    let response = request.send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        text,
    )
}

async fn ack(base: &str, invite: &str, setup: &str) -> u16 {
    reqwest::Client::new()
        .post(format!("{base}/membership/claim/ack"))
        .header("x-room-token", invite)
        .header("x-claim-id", setup)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn whoami(base: &str, header: (&str, &str)) -> (u16, Value) {
    let response = reqwest::Client::new()
        .get(format!("{base}/whoami"))
        .header(header.0, header.1)
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    (status, response.json().await.unwrap_or(Value::Null))
}

async fn lobby_can(port: u16, key: &str) -> bool {
    let mut request = format!("ws://127.0.0.1:{port}/lobby/ws")
        .into_client_request()
        .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.membership.{key}")).unwrap(),
    );
    tokio_tungstenite::connect_async(request).await.is_ok()
}

async fn credentials(base: &str, key: &str) -> Vec<Value> {
    reqwest::Client::new()
        .get(format!("{base}/profile/credentials"))
        .header("x-admin-token", key)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap()
}

async fn revoke_credential(base: &str, authority: &str, id: &str) -> u16 {
    reqwest::Client::new()
        .delete(format!("{base}/profile/credentials/{id}"))
        .header("x-admin-token", authority)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn take_session(base: &str, key: &str, body: Value) -> String {
    let response = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header("x-room-token", key)
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let session: Value = response.json().await.unwrap();
    session["session_token"].as_str().unwrap().to_string()
}

/// May this session read `parlor`?
async fn session_reads_parlor(base: &str, session: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("{base}/rooms/parlor/messages"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// The reviewer's case: set-up A is working; set-up B's answer is lost and
/// B retries, many times, at once, and across a restart. B ends with one
/// key, the count does not rise, and revoking A's key leaves B working.
#[tokio::test]
async fn two_set_ups_get_their_own_keys_and_a_lost_answer_retries_to_the_same_one() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("claims.sqlite3");
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;

    let (status, a, _) = claim(&base, &invite, Some(SETUP_A)).await;
    assert_eq!(status, 201);
    let key_a = a["membership_token"].as_str().unwrap().to_string();
    assert!(key_a.starts_with("mb_"));
    assert_ne!(
        key_a, original,
        "a claim never hands out the member's own key"
    );
    assert_eq!(a["member_id"], member_ref(&original));
    assert_eq!(a["name"], "worker");
    assert_eq!(a["ack_required"], true);
    assert!(a["credential_id"].as_str().unwrap().starts_with("cr_"));
    assert_eq!(
        whoami(&base, ("x-room-token", &key_a)).await.1["name"],
        "worker"
    );
    assert_eq!(ack(&base, &invite, SETUP_A).await, 200);

    // B's first answer is lost; B asks again, eight times at once.
    let (status, b, _) = claim(&base, &invite, Some(SETUP_B)).await;
    assert_eq!(status, 201);
    let key_b = b["membership_token"].as_str().unwrap().to_string();
    assert_ne!(key_b, key_a, "a second set-up gets a key of its own");
    let mut repeats = Vec::new();
    for _ in 0..8 {
        let base = base.clone();
        let invite = invite.clone();
        repeats.push(tokio::spawn(async move {
            claim(&base, &invite, Some(SETUP_B)).await
        }));
    }
    for repeat in repeats {
        let (status, again, _) = repeat.await.unwrap();
        assert_eq!(status, 200, "a repeat mints nothing");
        assert_eq!(again["membership_token"], key_b);
        assert_eq!(again["credential_id"], b["credential_id"]);
    }

    // A restart in the middle changes nothing.
    drop(guard);
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (status, again, _) = claim(&base, &invite, Some(SETUP_B)).await;
    assert_eq!(
        (status, again["membership_token"].clone()),
        (200, Value::from(key_b.clone()))
    );

    // B's nine repeats counted once: three more set-ups fill the invite's
    // five, and the sixth set-up is refused with what to do. Repeats of a
    // counted set-up are still answered.
    for setup in [SETUP_C, SETUP_D, SETUP_E] {
        assert_eq!(claim(&base, &invite, Some(setup)).await.0, 201);
    }
    let (status, _, text) = claim(&base, &invite, Some(SETUP_F)).await;
    assert_eq!(status, 429);
    assert!(
        text.contains("Ask the Master to re-issue the davet"),
        "{text}"
    );
    assert_eq!(claim(&base, &invite, Some(SETUP_B)).await.0, 200);
    assert_eq!(claim(&base, &invite, Some(SETUP_A)).await.0, 409);

    // A claimed key is a real credential: shown under Profile -> Keys with
    // its label, good for /whoami, the Lobby and a session...
    let keys = credentials(&base, &key_b).await;
    assert_eq!(
        keys.iter().filter(|k| k["label"] == CLAIMED_LABEL).count(),
        5,
        "{keys:?}"
    );
    assert!(keys
        .iter()
        .any(|k| k["id"] == b["credential_id"] && k["current"] == true));
    assert!(lobby_can(port, &key_b).await);
    take_session(&base, &key_b, serde_json::json!({ "name": "worker" })).await;

    // ...and revocable there. Revoking A's key ends A and nothing else.
    assert_eq!(
        revoke_credential(&base, &key_b, a["credential_id"].as_str().unwrap()).await,
        204
    );
    assert_eq!(whoami(&base, ("x-room-token", &key_a)).await.0, 401);
    assert!(!lobby_can(port, &key_a).await);
    assert_eq!(whoami(&base, ("x-room-token", &key_b)).await.0, 200);
    assert!(lobby_can(port, &key_b).await);
    assert_eq!(whoami(&base, ("x-room-token", &original)).await.0, 200);
    assert_eq!(
        reqwest::Client::new()
            .get(format!("{base}/rooms/parlor/messages"))
            .header("x-room-token", &invite)
            .send()
            .await
            .unwrap()
            .status(),
        200,
        "nothing a claim did touched the invite"
    );

    // B acknowledges: from then on it is told the key was delivered.
    assert_eq!(ack(&base, &invite, SETUP_B).await, 200);
    assert_eq!(
        ack(&base, &invite, SETUP_B).await,
        200,
        "an ACK is idempotent"
    );
    let (status, _, text) = claim(&base, &invite, Some(SETUP_B)).await;
    assert_eq!(status, 409);
    assert!(text.starts_with("already delivered"), "{text}");
}

/// What an installed older kit sends: no claim id, and one of three request
/// shapes (connect.sh: JSON content type, no body; listen.py: an empty body;
/// setup.ps1: `{}`). Every call is a set-up of its own and gets a working key
/// under `membership_token`, with no retry guarantee; the sixth is refused
/// with what to do; a new invite for the member starts a fresh count. In a
/// persistent and in a memory-only Building alike.
#[tokio::test]
async fn an_older_kit_claims_a_key_per_call_until_the_cap_and_a_new_invite_counts_afresh() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("older-kit.sqlite3");
    for env in [persistent(&db), vec![("REQUIRE_INVITE", "1".to_string())]] {
        let (port, _guard) = spawn_server_env(ROOT, &env).await;
        let base = format!("http://127.0.0.1:{port}");
        let (original, invite) = seated(&base).await;
        let client = reqwest::Client::new();
        let mut keys = std::collections::HashSet::new();
        for call in 0..6 {
            let request = client
                .post(format!("{base}/membership/claim"))
                .header("x-room-token", &invite);
            let request = match call % 3 {
                0 => request.header("content-type", "application/json"),
                1 => request.body(Vec::<u8>::new()),
                _ => request
                    .header("content-type", "application/json")
                    .body("{}"),
            };
            let response = request.send().await.unwrap();
            let status = response.status().as_u16();
            let text = response.text().await.unwrap();
            if call == 5 {
                assert_eq!(status, 429, "{env:?}");
                assert!(text.contains("Ask the Master"), "{text}");
                continue;
            }
            assert_eq!(status, 201, "{env:?}: {text}");
            let body: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(body["ack_required"], false);
            let key = body["membership_token"].as_str().unwrap().to_string();
            assert_ne!(key, original);
            let (status, who) = whoami(&base, ("x-room-token", &key)).await;
            assert_eq!((status, who["kind"].as_str()), (200, Some("member")));
            assert!(lobby_can(port, &key).await);
            assert!(keys.insert(key), "every call is a set-up of its own");
        }

        // The Master gives the member a new invite: it has a count of its own.
        let released = client
            .post(format!("{base}/rooms/parlor/release"))
            .header("x-room-token", &invite)
            .send()
            .await
            .unwrap();
        assert_eq!(released.status(), 204);
        let called: Value = client
            .post(format!("{base}/rooms/parlor/call"))
            .header("x-admin-token", ROOT)
            .json(&serde_json::json!({ "name": "worker" }))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let fresh = called["token"].as_str().unwrap();
        assert_ne!(fresh, invite);
        assert_eq!(claim(&base, fresh, None).await.0, 201, "{env:?}");
        assert_eq!(claim(&base, fresh, Some(SETUP_A)).await.0, 201, "{env:?}");
    }
}

/// A database that cannot be read for a moment is not an unknown set-up: the
/// kit keeps its claim id on a 503 and tries again, and the set-up carries
/// on once the database answers.
#[tokio::test]
async fn a_storage_error_on_a_claim_or_its_ack_is_a_retryable_503() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("claim-503.sqlite3");
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (_original, invite) = seated(&base).await;
    let (_, first, _) = claim(&base, &invite, Some(SETUP_A)).await;

    let side = rusqlite::Connection::open(&db).unwrap();
    side.execute_batch("ALTER TABLE membership_claims RENAME TO membership_claims_away")
        .unwrap();
    assert_eq!(ack(&base, &invite, SETUP_A).await, 503);
    assert_eq!(claim(&base, &invite, Some(SETUP_A)).await.0, 503);
    assert_eq!(claim(&base, &invite, Some(SETUP_B)).await.0, 503);
    side.execute_batch("ALTER TABLE membership_claims_away RENAME TO membership_claims")
        .unwrap();

    let (status, again, _) = claim(&base, &invite, Some(SETUP_A)).await;
    assert_eq!(
        (status, &again["membership_token"]),
        (200, &first["membership_token"])
    );
    assert_eq!(ack(&base, &invite, SETUP_A).await, 200);
    assert_eq!(ack(&base, &invite, SETUP_B).await, 404, "no such set-up");
    assert_eq!(
        reqwest::Client::new()
            .post(format!("{base}/membership/claim/ack"))
            .header("x-room-token", &invite)
            .send()
            .await
            .unwrap()
            .status(),
        400,
        "an ACK names its set-up"
    );
}

/// Ids and hashes are not credentials, and nothing but a kit-shaped claim id
/// is a claim id: each is refused, and refusing changes nothing.
#[tokio::test]
async fn derived_values_are_neither_credentials_nor_claim_ids() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("derived.sqlite3");
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;
    let (_, claimed, _) = claim(&base, &invite, Some(SETUP_A)).await;
    let key = claimed["membership_token"].as_str().unwrap().to_string();
    let profile: Value = reqwest::Client::new()
        .get(format!("{base}/profile"))
        .header("x-admin-token", &key)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let principal = profile["principal"]["id"].as_str().unwrap().to_string();
    assert!(principal.starts_with("pr_"));
    let sha = |value: &str| {
        use sha2::{Digest, Sha256};
        hex(&Sha256::digest(value.as_bytes()))
    };
    let derived = [
        sha(&key),
        sha(&original),
        sha(&invite),
        sha(SETUP_A),
        SETUP_A.to_string(),
        member_ref(&original),
        invite_ref(&invite),
        claimed["credential_id"].as_str().unwrap().to_string(),
        principal,
    ];
    let client = reqwest::Client::new();
    for value in &derived {
        assert_eq!(
            whoami(&base, ("x-room-token", value)).await.0,
            401,
            "{value}"
        );
        assert!(!lobby_can(port, value).await, "{value}");
        let session = client
            .post(format!("{base}/sessions"))
            .header("x-room-token", value)
            .json(&serde_json::json!({ "name": "worker" }))
            .send()
            .await
            .unwrap();
        assert_eq!(session.status(), 401, "{value}");
        assert_eq!(claim(&base, value, Some(SETUP_B)).await.0, 401, "{value}");
    }
    // Where a claim id is expected, a key, an invite, a hash or an id is not one.
    for value in derived
        .iter()
        .take(4)
        .chain(derived.iter().skip(5))
        .chain([&key, &invite])
    {
        assert_eq!(claim(&base, &invite, Some(value)).await.0, 400, "{value}");
    }
    // A membership key cannot claim another.
    for member_key in [&key, &original] {
        let (status, _, text) = claim(&base, member_key, Some(SETUP_B)).await;
        assert_eq!(status, 400, "{text}");
    }
    // Nothing above minted: the invite still has room for exactly four more.
    for setup in [SETUP_B, SETUP_C, SETUP_D, SETUP_E] {
        assert_eq!(claim(&base, &invite, Some(setup)).await.0, 201);
    }
    assert_eq!(claim(&base, &invite, Some(SETUP_F)).await.0, 429);
}

/// Door B: revoking one credential ends the sessions taken with it and
/// nothing else. An invite session belongs to its invite, not to the member's
/// original key: it outlives that key's revocation and ends with the invite.
#[tokio::test]
async fn revoking_one_credential_ends_only_its_sessions_and_invite_sessions_outlive_the_original_key(
) {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("door-b.sqlite3");
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;
    let spare: Value = reqwest::Client::new()
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", &original)
        .json(&serde_json::json!({ "label": "Worker spare" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let spare = spare["secret"].as_str().unwrap().to_string();
    let invite_session = take_session(
        &base,
        &invite,
        serde_json::json!({ "name": "worker", "loca": "parlor" }),
    )
    .await;
    let original_session =
        take_session(&base, &original, serde_json::json!({ "name": "worker" })).await;
    assert_eq!(session_reads_parlor(&base, &invite_session).await, 200);

    let keys = credentials(&base, &spare).await;
    let original_id = keys
        .iter()
        .find(|k| k["label"] == "Member access")
        .and_then(|k| k["id"].as_str())
        .unwrap()
        .to_string();
    assert_eq!(revoke_credential(&base, &spare, &original_id).await, 204);

    assert_eq!(whoami(&base, ("x-room-token", &original)).await.0, 401);
    assert_eq!(
        whoami(&base, ("x-session-token", &original_session))
            .await
            .0,
        401,
        "a session taken with the revoked key ends with it"
    );
    assert_eq!(
        session_reads_parlor(&base, &invite_session).await,
        200,
        "an invite session is not bound to the member's original key"
    );
    let later = take_session(
        &base,
        &invite,
        serde_json::json!({ "name": "worker", "loca": "parlor" }),
    )
    .await;
    assert_eq!(session_reads_parlor(&base, &later).await, 200);

    // A claimed key's session ends with that key alone.
    let (_, claimed, _) = claim(&base, &invite, Some(SETUP_A)).await;
    let claimed_key = claimed["membership_token"].as_str().unwrap().to_string();
    let claimed_session =
        take_session(&base, &claimed_key, serde_json::json!({ "name": "worker" })).await;
    assert_eq!(
        revoke_credential(&base, &spare, claimed["credential_id"].as_str().unwrap()).await,
        204
    );
    assert_eq!(
        whoami(&base, ("x-session-token", &claimed_session)).await.0,
        401
    );
    assert_eq!(session_reads_parlor(&base, &invite_session).await, 200);
    assert_eq!(whoami(&base, ("x-room-token", &spare)).await.0, 200);

    // The invite session ends with its invite.
    let released = reqwest::Client::new()
        .post(format!("{base}/rooms/parlor/release"))
        .header("x-room-token", &invite)
        .send()
        .await
        .unwrap();
    assert_eq!(released.status(), 204);
    assert_eq!(session_reads_parlor(&base, &invite_session).await, 401);
    assert_eq!(session_reads_parlor(&base, &later).await, 401);
    // An invite that ended claims nothing; its set-up can still acknowledge
    // (the ACK only ever clears a copy).
    assert_eq!(claim(&base, &invite, Some(SETUP_B)).await.0, 401);
    assert_eq!(claim(&base, &invite, Some(SETUP_A)).await.0, 401);
    assert_eq!(ack(&base, &invite, SETUP_A).await, 200);
}

/// Door A: the Master revokes the membership, and every credential of the
/// member goes with it, claimed keys and Profile keys included, now and
/// after a restart.
#[tokio::test]
async fn revoking_the_membership_revokes_every_credential_of_the_member() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("door-a.sqlite3");
    let (port, guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;
    let spare: Value = reqwest::Client::new()
        .post(format!("{base}/profile/credentials"))
        .header("x-admin-token", &original)
        .json(&serde_json::json!({ "label": "Worker spare" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let spare = spare["secret"].as_str().unwrap().to_string();
    let (status, claimed, _) = claim(&base, &invite, Some(SETUP_A)).await;
    assert_eq!(status, 201);
    let live = vec![
        original.clone(),
        spare.clone(),
        claimed["membership_token"].as_str().unwrap().to_string(),
    ];
    let sessions = {
        let mut sessions = Vec::new();
        for key in &live {
            sessions.push(take_session(&base, key, serde_json::json!({ "name": "worker" })).await);
        }
        sessions
    };

    let revoked = reqwest::Client::new()
        .delete(format!("{base}/members/{}", member_ref(&original)))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(revoked.status(), 200);
    for key in &live {
        assert_eq!(whoami(&base, ("x-room-token", key)).await.0, 401);
        assert!(!lobby_can(port, key).await);
    }
    for session in &sessions {
        assert_eq!(whoami(&base, ("x-session-token", session)).await.0, 401);
    }
    let unrevoked: i64 = rusqlite::Connection::open(&db)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM credentials WHERE revoked_at IS NULL AND principal_id IN \
             (SELECT principal_id FROM credentials WHERE label = 'Member access')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(unrevoked, 0, "every credential of the member is revoked");

    drop(guard);
    let (_port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{_port}");
    for key in &live {
        assert_eq!(whoami(&base, ("x-room-token", key)).await.0, 401);
    }
}

/// Management URLs name a member or an invite by its id. A raw key in the
/// path is an unknown reference: 404, and nothing is revoked.
#[tokio::test]
async fn a_raw_key_in_a_management_url_is_an_unknown_reference() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("refs.sqlite3");
    let (_port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{_port}");
    let (original, invite_key) = seated(&base).await;
    let client = reqwest::Client::new();
    let member = client
        .delete(format!("{base}/members/{original}"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(member.status(), 404);
    let invite = client
        .delete(format!("{base}/rooms/parlor/invites/{invite_key}"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(invite.status(), 404);
    assert_eq!(whoami(&base, ("x-room-token", &original)).await.0, 200);
    assert_eq!(
        whoami(&base, ("x-room-token", &invite_key)).await.1["kind"],
        "davet"
    );
    // By id, the same requests work.
    let invite = client
        .delete(format!(
            "{base}/rooms/parlor/invites/{}",
            invite_ref(&invite_key)
        ))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(invite.status(), 200);
    let member = client
        .delete(format!("{base}/members/{}", member_ref(&original)))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap();
    assert_eq!(member.status(), 200);
}

/// No answer an issuer sees, and no frame a member's sockets receive,
/// carries the member's own key or a claimed one.
#[tokio::test]
async fn no_answer_or_frame_hands_out_a_member_key() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("frames.sqlite3");
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;
    let (_, claimed, _) = claim(&base, &invite, Some(SETUP_A)).await;
    let key = claimed["membership_token"].as_str().unwrap().to_string();
    let client = reqwest::Client::new();
    let mut seen = Vec::new();

    let mut lobby = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/lobby/ws"),
        &["loca.v1".into(), format!("loca.membership.{key}")],
    )
    .await;
    seen.push(wait_for(&mut lobby, |v| v["t"] == "lobby_ready").await);
    seen.push(wait_for(&mut lobby, |v| v["t"] == "called").await);
    for (path, body) in [
        ("/rooms", serde_json::json!({ "name": "second" })),
        (
            "/rooms/second/invites",
            serde_json::json!({ "name": "worker" }),
        ),
    ] {
        let answer: Value = client
            .post(format!("{base}{path}"))
            .header("x-admin-token", ROOT)
            .json(&body)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        seen.push(answer);
    }
    seen.push(wait_for(&mut lobby, |v| v["t"] == "called").await);
    let readmitted: Value = client
        .post(format!("{base}/members"))
        .header("x-admin-token", ROOT)
        .json(&serde_json::json!({ "name": "worker", "kind": "agent" }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    seen.push(readmitted);
    for path in ["/rooms/parlor/invites", "/members", "/residents"] {
        seen.push(
            client
                .get(format!("{base}{path}"))
                .header("x-admin-token", ROOT)
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap(),
        );
    }
    let mut room = connect_ws_protocols(
        format!("ws://127.0.0.1:{port}/ws?room=parlor&name=worker&type=agent"),
        &["loca.v1".into(), format!("loca.room.{invite}")],
    )
    .await;
    seen.push(wait_for(&mut room, |v| v["t"] == "history").await);
    seen.push(wait_for(&mut room, |v| v["t"] == "members").await);
    for value in seen {
        let text = value.to_string();
        assert!(
            !text.contains(&original) && !text.contains(&key) && !text.contains("mb_"),
            "a member key was handed out: {text}"
        );
    }
}

/// The key ledger's row for a member counts a use of any of its keys: an
/// agent that signs in only with the key its invite claimed is in use, not
/// "never used".
#[tokio::test]
async fn a_member_signed_in_only_with_a_claimed_key_shows_in_use_in_the_ledger() {
    let directory = tempfile::tempdir().unwrap();
    let db = directory.path().join("ledger.sqlite3");
    let (port, _guard) = spawn_server_env(ROOT, &persistent(&db)).await;
    let base = format!("http://127.0.0.1:{port}");
    let (original, invite) = seated(&base).await;
    let (_, claimed, _) = claim(&base, &invite, Some(SETUP_A)).await;
    let key = claimed["membership_token"].as_str().unwrap();
    assert_eq!(whoami(&base, ("x-room-token", key)).await.0, 200);
    let ledger: Vec<Value> = reqwest::Client::new()
        .get(format!("{base}/keys"))
        .header("x-admin-token", ROOT)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let row = ledger
        .iter()
        .find(|row| row["id"] == member_ref(&original))
        .expect("the member's ledger row");
    assert_eq!(row["state"], "active", "{row}");
    assert!(row["last_used_at"].as_u64().is_some());
}
