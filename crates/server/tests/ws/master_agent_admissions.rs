//! A Master Agent admits agents into the locas it opened, in one call that
//! returns the new member's key once (retried with the same claim id until
//! acknowledged), and calls an agent it admitted into another loca of its own.
//! Never a human, never a loca it did not open, never past the Master's
//! limits; every record says on whose behalf (owner's decisions, 2026-11-18).
use super::master_agent::{building, json, send, session, Building, OFF};
use super::*;

const CLAIM_A: &str = "cl_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CLAIM_B: &str = "cl_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

async fn appointed(b: &Building) -> String {
    assert_eq!(b.setting(true).await.0, 200);
    let (status, v) = b.appoint("automaton").await;
    assert_eq!(status, 201, "{v}");
    session(
        &b.base,
        ("x-admin-token", v["code"].as_str().unwrap()),
        "automaton",
        "agent",
    )
    .await
}

async fn open(b: &Building, ma: &str, name: &str) {
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/rooms"),
        Some(("x-session-token", ma)),
        Some(serde_json::json!({ "name": name, "group": b.floor })),
    )
    .await;
    assert_eq!(status, 201, "{text}");
}

async fn admit(b: &Building, ma: &str, room: &str, body: Value) -> (u16, String) {
    send(
        reqwest::Method::POST,
        b.url(&format!("/rooms/{room}/admissions")),
        Some(("x-session-token", ma)),
        Some(body),
    )
    .await
}

fn claim(n: usize) -> String {
    format!("cl_{n:0>40}")
}

#[tokio::test]
async fn one_call_admits_an_agent_seats_it_and_returns_its_key_once() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;

    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "alpha-one", "claim_id": CLAIM_A }),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    let v = json(&text);
    let key = v["key"].as_str().unwrap().to_string();
    assert!(key.starts_with("mb_"), "{v}");
    assert_eq!(v["name"], "alpha-one");
    assert_eq!(v["loca"], "alpha");
    assert_eq!(v["group"], b.floor.as_str());
    let member_id = v["member_id"].as_str().unwrap().to_string();
    assert!(v["principal_id"].as_str().unwrap().starts_with("pr_"));
    assert!(v["invite_id"].as_str().unwrap().starts_with("ivid_"));

    // The key works: an agent member, seated in alpha, on the floor.
    let (status, text) = send(
        reqwest::Method::GET,
        b.url("/whoami"),
        Some(("x-room-token", &key)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["member_kind"], "agent");
    assert_eq!(json(&text)["locas"], serde_json::json!(["alpha"]));
    let agent = session(&b.base, ("x-room-token", &key), "alpha-one", "agent").await;
    let (status, _) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &agent)),
        None,
    )
    .await;
    assert_eq!(status, 200, "the agent enters the loca it was called into");

    // The same claim gets the same key until it is acknowledged; another
    // claim for the same name gets no key at all.
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "alpha-one", "claim_id": CLAIM_A }),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["key"], key.as_str());
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "alpha-one", "claim_id": CLAIM_B }),
    )
    .await;
    assert_eq!(status, 409, "{text}");
    assert!(!text.contains(&key));
    let (status, text) = send(
        reqwest::Method::POST,
        b.url(&format!("/rooms/alpha/admissions/{member_id}/ack")),
        Some(("x-session-token", &ma)),
        Some(serde_json::json!({ "claim_id": CLAIM_A })),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "alpha-one", "claim_id": CLAIM_A }),
    )
    .await;
    assert_eq!(
        status, 409,
        "acknowledged: the key is not handed out again: {text}"
    );
    assert!(!text.contains(&key));

    // Every record names it, on whose behalf.
    let (_, text) = b.as_master(reqwest::Method::GET, "/members", None).await;
    let row = json(&text)
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "alpha-one")
        .cloned()
        .unwrap();
    assert_eq!(row["admitted_by"], "master-agent:automaton");
    assert_eq!(row["kind"], "agent");
    let (_, ledger) = b.as_master(reqwest::Method::GET, "/keys", None).await;
    assert!(!ledger.contains(&key), "the ledger keeps no key");
    let ledger = json(&ledger);
    for kind in ["member", "davet"] {
        let entry = ledger
            .as_array()
            .unwrap()
            .iter()
            .find(|k| k["kind"] == kind && k["for_name"] == "alpha-one")
            .unwrap_or_else(|| panic!("{kind} row: {ledger}"));
        assert_eq!(entry["issued_by"], "master-agent:automaton", "{kind}");
    }
    let (_, text) = b
        .as_master(
            reqwest::Method::GET,
            &format!("/groups/{}/log", b.floor),
            None,
        )
        .await;
    let log = json(&text);
    let added = log
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "member_added" && e["subject"] == "alpha-one")
        .unwrap_or_else(|| panic!("{log}"));
    assert_eq!(added["by"], "automaton");
    assert_eq!(added["on_behalf_of"]["appointed_by"], "Master");
}

#[tokio::test]
async fn never_a_human_never_a_loca_it_did_not_open() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;

    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "a-person", "kind": "user", "claim_id": claim(1) }),
    )
    .await;
    assert_eq!(status, 400, "never a human: {text}");
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "opal", "claim_id": claim(2) }),
    )
    .await;
    assert_eq!(
        status, 409,
        "an existing member is not admitted again: {text}"
    );
    let (status, text) = admit(&b, &ma, "alpha", serde_json::json!({ "name": "no-claim" })).await;
    assert_eq!(status, 400, "a claim id is required: {text}");

    for room in [
        b.floor_loca.as_str(),
        "ground",
        "lobby",
        "iye",
        b.dm.as_str(),
    ] {
        let (status, text) = admit(
            &b,
            &ma,
            room,
            serde_json::json!({ "name": "stray-agent", "claim_id": claim(3) }),
        )
        .await;
        assert!(matches!(status, 401 | 403 | 404), "{room}: {status} {text}");
    }
    let (_, text) = b.as_master(reqwest::Method::GET, "/members", None).await;
    assert!(!text.contains("stray-"), "nobody was admitted: {text}");

    // A floor taken off the list takes no new admission.
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "floors": [] })),
        )
        .await;
    assert_eq!(status, 200);
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "late-agent", "claim_id": claim(4) }),
    )
    .await;
    assert_eq!(status, 403, "{text}");

    // Off: the one sentence.
    assert_eq!(b.setting(false).await.0, 200);
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "off-agent", "claim_id": claim(5) }),
    )
    .await;
    assert_eq!((status, text.as_str()), (403, OFF));
}

#[tokio::test]
async fn it_calls_only_agents_it_admitted_into_locas_it_opened() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;
    open(&b, &ma, "beta").await;
    let (status, _) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "alpha-one", "claim_id": CLAIM_A }),
    )
    .await;
    assert_eq!(status, 201);

    let call = |room: &str, name: &str| {
        let url = b.url(&format!("/rooms/{room}/call"));
        let ma = ma.clone();
        let name = name.to_string();
        async move {
            send(
                reqwest::Method::POST,
                url,
                Some(("x-session-token", &ma)),
                Some(serde_json::json!({ "name": name })),
            )
            .await
        }
    };
    let (status, text) = call("beta", "alpha-one").await;
    assert_eq!(status, 201, "{text}");
    let (status, text) = call("beta", "envoy").await;
    assert_eq!(status, 403, "an agent it did not admit: {text}");
    for room in [b.floor_loca.as_str(), "ground", "lobby"] {
        let (status, text) = call(room, "alpha-one").await;
        assert!(matches!(status, 401 | 403 | 404), "{room}: {status} {text}");
    }
    let (_, text) = send(
        reqwest::Method::GET,
        b.url("/rooms/beta/invites"),
        Some(("x-session-token", &b.master)),
        None,
    )
    .await;
    let invite = json(&text)
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["name"] == "alpha-one")
        .cloned()
        .unwrap();
    assert_eq!(invite["issued_by"], "master-agent:automaton");

    // Its invites rank below a Smaster's: the floor's founder may end one.
    let (status, text) = send(
        reqwest::Method::DELETE,
        b.url(&format!(
            "/rooms/beta/invites/{}",
            invite["id"].as_str().unwrap()
        )),
        Some(("x-session-token", &b.smaster)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{text}");
}

#[tokio::test]
async fn the_masters_limits_bound_how_many_agents_it_admits() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({
                "on": true,
                "limits": { "admissions_per_hour": 2, "live_agents": 3 },
            })),
        )
        .await;
    assert_eq!(status, 200);
    for n in 1..=2 {
        let (status, text) = admit(
            &b,
            &ma,
            "alpha",
            serde_json::json!({ "name": format!("agent-{n}"), "claim_id": claim(n) }),
        )
        .await;
        assert_eq!(status, 201, "{text}");
    }
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "agent-3", "claim_id": claim(3) }),
    )
    .await;
    assert_eq!(status, 429, "the hour's limit: {text}");

    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "limits": { "admissions_per_hour": 10 } })),
        )
        .await;
    assert_eq!(status, 200);
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "agent-3", "claim_id": claim(3) }),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "agent-4", "claim_id": claim(4) }),
    )
    .await;
    assert_eq!(status, 429, "the live agents' limit: {text}");

    // The Master ends one of them: room for one more.
    let (_, text) = b.as_master(reqwest::Method::GET, "/members", None).await;
    let id = json(&text)
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["name"] == "agent-1")
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (status, _) = b
        .as_master(reqwest::Method::DELETE, &format!("/members/{id}"), None)
        .await;
    assert_eq!(status, 200);
    let (status, text) = admit(
        &b,
        &ma,
        "alpha",
        serde_json::json!({ "name": "agent-4", "claim_id": claim(4) }),
    )
    .await;
    assert_eq!(status, 201, "{text}");
}

/// Review H1: its key presented as the room credential (`x-room-token`) is
/// still the Master Agent. It used to pass the fence and mint an anonymous
/// session under any name, which read and spoke in the Lobby and outlived
/// the switch and a revocation.
#[tokio::test]
async fn its_key_as_the_room_credential_is_still_the_master_agent() {
    let b = building().await;
    assert_eq!(b.setting(true).await.0, 200);
    let (status, v) = b.appoint("automaton").await;
    assert_eq!(status, 201, "{v}");
    let key = v["code"].as_str().unwrap().to_string();
    let room_key = ("x-room-token", key.as_str());

    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/sessions"),
        Some(room_key),
        Some(serde_json::json!({ "name": "opal", "kind": "user" })),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    let token = json(&text)["session_token"].as_str().unwrap().to_string();
    let (_, who) = send(
        reqwest::Method::GET,
        b.url("/whoami"),
        Some(("x-session-token", &token)),
        None,
    )
    .await;
    assert!(!who.contains("opal"), "the body named the session: {who}");
    // The Lobby is not a loca it opened.
    for credential in [("x-session-token", token.as_str()), room_key] {
        let (status, text) = send(
            reqwest::Method::GET,
            b.url("/rooms/lobby/messages"),
            Some(credential),
            None,
        )
        .await;
        assert_eq!(status, 404, "{}: {text}", credential.0);
        let (status, text) = send(
            reqwest::Method::POST,
            b.url("/rooms/lobby/messages"),
            Some(credential),
            Some(serde_json::json!({ "sender": "opal", "sender_type": "user", "text": "hi" })),
        )
        .await;
        assert_eq!(status, 404, "{}: {text}", credential.0);
    }
    // Off means off on this path too.
    assert_eq!(b.setting(false).await.0, 200);
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/sessions"),
        Some(room_key),
        Some(serde_json::json!({ "name": "opal", "kind": "user" })),
    )
    .await;
    assert_eq!(status, 403, "{text}");
    assert!(text.contains(OFF), "{text}");
    let (status, _) = send(
        reqwest::Method::GET,
        b.url("/rooms/lobby/messages"),
        Some(("x-session-token", &token)),
        None,
    )
    .await;
    assert_eq!(status, 403);
}

/// Review L1: a loca it did not open answers exactly as one that does not
/// exist, a Ground-floor loca included.
#[tokio::test]
async fn a_loca_it_did_not_open_answers_as_one_that_is_not_there() {
    let b = building().await;
    let ma = appointed(&b).await;
    let get = |path: &str| {
        let url = b.url(path);
        let ma = ma.clone();
        async move {
            send(
                reqwest::Method::GET,
                url,
                Some(("x-session-token", &ma)),
                None,
            )
            .await
        }
    };
    let nothing = get("/rooms/no-such-loca-here/messages").await;
    assert_eq!(nothing.0, 404);
    for room in ["ground", b.floor_loca.as_str(), "lobby"] {
        assert_eq!(
            get(&format!("/rooms/{room}/messages")).await,
            nothing,
            "{room}"
        );
    }
    assert_eq!(
        b.ws_status("ground", &ma).await,
        b.ws_status("no-such-loca-here", &ma).await
    );
    // A repeated or encoded `room` is read as the handler reads it.
    open(&b, &ma, "mine").await;
    for trick in ["mine&room=ground", "ground&r%6Fom=mine&r%6Fom=ground"] {
        assert_eq!(
            b.ws_status(trick, &ma).await,
            b.ws_status("no-such-loca-here", &ma).await,
            "{trick}"
        );
    }
}

/// Review M2: its agents are its own by principal, not by the name it wears.
/// A Master Agent appointed later under a revoked one's name calls none of
/// the revoked one's agents and inherits none of its counts.
#[tokio::test]
async fn a_new_master_agent_under_an_old_name_inherits_nothing() {
    let b = building().await;
    let old = appointed(&b).await;
    open(&b, &old, "alpha").await;
    let (status, _) = admit(
        &b,
        &old,
        "alpha",
        serde_json::json!({ "name": "old-one", "claim_id": CLAIM_A }),
    )
    .await;
    assert_eq!(status, 201);
    let (_, text) = b
        .as_master(reqwest::Method::GET, "/master-agents", None)
        .await;
    let id = json(&text)[0]["id"].as_str().unwrap().to_string();
    let (status, text) = b
        .as_master(
            reqwest::Method::DELETE,
            &format!("/master-agents/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{text}");

    let new = appointed(&b).await;
    open(&b, &new, "gamma").await;
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/rooms/gamma/call"),
        Some(("x-session-token", &new)),
        Some(serde_json::json!({ "name": "old-one" })),
    )
    .await;
    assert_eq!(status, 403, "the revoked one's agent: {text}");
}

/// Review M3: replaying a claim hands the key back but does not seat the
/// agent again once its admission was seated: an agent the Master sent out
/// of the loca stays out.
#[tokio::test]
async fn a_replayed_claim_does_not_seat_again_an_agent_the_master_removed() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;
    let body = serde_json::json!({ "name": "seat-one", "claim_id": CLAIM_B });
    let (status, text) = admit(&b, &ma, "alpha", body.clone()).await;
    assert_eq!(status, 201, "{text}");
    let key = json(&text)["key"].as_str().unwrap().to_string();
    let invite_of = || async {
        let (_, text) = b
            .as_master(reqwest::Method::GET, "/rooms/alpha/invites", None)
            .await;
        json(&text)
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["name"] == "seat-one")
            .and_then(|i| i["id"].as_str().map(str::to_string))
    };
    let invite = invite_of().await.expect("seated");
    let (status, text) = b
        .as_master(
            reqwest::Method::DELETE,
            &format!("/rooms/alpha/invites/{invite}"),
            None,
        )
        .await;
    assert_eq!(status, 200, "{text}");

    let (status, text) = admit(&b, &ma, "alpha", body).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["key"], key.as_str());
    assert_eq!(invite_of().await, None, "the replay seated it again");
}

/// Review M1: the limits hold under concurrent admissions: they are counted,
/// admitted and recorded as one step.
#[tokio::test]
async fn concurrent_admissions_do_not_pass_the_same_count() {
    let b = building().await;
    let ma = appointed(&b).await;
    open(&b, &ma, "alpha").await;
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "limits": { "live_agents": 2 } })),
        )
        .await;
    assert_eq!(status, 200);
    let tries: Vec<_> = (10..18)
        .map(|n| {
            let url = b.url("/rooms/alpha/admissions");
            let ma = ma.clone();
            tokio::spawn(async move {
                send(
                    reqwest::Method::POST,
                    url,
                    Some(("x-session-token", &ma)),
                    Some(serde_json::json!({ "name": format!("racer-{n}"), "claim_id": claim(n) })),
                )
                .await
                .0
            })
        })
        .collect();
    let mut statuses = Vec::new();
    for t in tries {
        statuses.push(t.await.unwrap());
    }
    let admitted = statuses.iter().filter(|s| **s == 201).count();
    assert!(admitted <= 2, "past the live agents' limit: {statuses:?}");
    assert!(
        statuses.iter().all(|s| matches!(*s, 201 | 429)),
        "{statuses:?}"
    );
}

/// Review L2: the profile directory shows a Master Agent to the Master only.
#[tokio::test]
async fn a_smaster_does_not_see_master_agents_among_the_profiles() {
    let b = building().await;
    appointed(&b).await;
    let (_, by_master) = b.as_master(reqwest::Method::GET, "/profiles", None).await;
    assert!(by_master.contains("automaton"), "{by_master}");
    let (status, by_smaster) = send(
        reqwest::Method::GET,
        b.url("/profiles"),
        Some(("x-session-token", &b.smaster)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{by_smaster}");
    assert!(!by_smaster.contains("automaton"), "{by_smaster}");
}
