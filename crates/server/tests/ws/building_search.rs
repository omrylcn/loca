//! Building-wide search (`GET /search`): the words of exactly the locas the
//! caller may enter, judged with the same door as `GET /rooms`. PRINCIPLES:
//! an invite opens one loca; a direct message belongs to its two people.
use super::*;

const MASTER: &str = "search-master-key";

async fn member(base: &str, name: &str) -> String {
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/members"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name, "kind": "user" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    v["token"].as_str().unwrap().to_string()
}

async fn seat(base: &str, room: &str, name: &str) {
    reqwest::Client::new()
        .post(format!("{base}/rooms/{room}/invites"))
        .header("x-admin-token", MASTER)
        .json(&serde_json::json!({ "name": name }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap();
}

async fn session(base: &str, key: &str, name: &str, kind: &str) -> String {
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/sessions"))
        .header("x-room-token", key)
        .json(&serde_json::json!({ "name": name, "kind": kind }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    v["session_token"].as_str().unwrap().to_string()
}

async fn say(base: &str, room: &str, session: &str, text: &str) -> u64 {
    let v: Value = reqwest::Client::new()
        .post(format!("{base}/rooms/{room}/messages"))
        .header("x-session-token", session)
        .json(&serde_json::json!({ "sender": "x", "sender_type": "user", "text": text }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    v["id"].as_u64().unwrap()
}

async fn search(base: &str, session: &str, query: &str) -> (u16, Value) {
    let r = reqwest::Client::new()
        .get(format!("{base}/search?{query}"))
        .header("x-session-token", session)
        .send()
        .await
        .unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap_or_default();
    (
        status,
        serde_json::from_str(&text).unwrap_or(Value::String(text)),
    )
}

fn rooms_of(hits: &Value) -> Vec<String> {
    hits.as_array()
        .unwrap()
        .iter()
        .map(|h| h["room"].as_str().unwrap().to_string())
        .collect()
}

#[tokio::test]
async fn building_search_reads_only_the_locas_the_caller_may_enter() {
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("building-search.db");
    let (port, _guard) = spawn_server_env(
        MASTER,
        &[
            ("DB_PATH", db.to_str().unwrap().into()),
            ("REQUIRE_SESSIONS", "1".into()),
            // A closed Building: a member reaches a loca by its invite only.
            ("REQUIRE_INVITE", "1".into()),
        ],
    )
    .await;
    let base = format!("http://127.0.0.1:{port}");

    // ayse sits in alpha and beta; mehmet and can share gamma, and keep a
    // direct message ayse is no part of.
    for room in ["alpha", "beta", "gamma"] {
        create_test_room(port, MASTER, room).await;
    }
    let ayse_key = member(&base, "ayse").await;
    let mehmet_key = member(&base, "mehmet").await;
    let can_key = member(&base, "can").await;
    seat(&base, "alpha", "ayse").await;
    seat(&base, "beta", "ayse").await;
    seat(&base, "gamma", "mehmet").await;
    seat(&base, "gamma", "can").await;
    let ayse = session(&base, &ayse_key, "ayse", "user").await;
    let mehmet = session(&base, &mehmet_key, "mehmet", "user").await;
    let _can = session(&base, &can_key, "can", "user").await;

    let in_alpha = say(&base, "alpha", &ayse, "the Needle sits in alpha").await;
    let in_beta = say(&base, "beta", &ayse, "a needle in beta too").await;
    say(&base, "gamma", &mehmet, "gamma keeps its own needle").await;
    let opened: Value = reqwest::Client::new()
        .post(format!("{base}/birebirs"))
        .header("x-session-token", &mehmet)
        .json(&serde_json::json!({ "name": "can" }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let dm = opened["birebir"]["room"].as_str().unwrap().to_string();
    let in_dm = say(&base, &dm, &mehmet, "a private needle between two").await;
    say(&base, "alpha", &ayse, "nothing to find here").await;

    // Two locas the caller has, newest first, and nothing else.
    let (status, hits) = search(&base, &ayse, "q=needle").await;
    assert_eq!(status, 200, "{hits}");
    assert_eq!(rooms_of(&hits), ["beta", "alpha"], "{hits}");
    let ids: Vec<u64> = hits
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, [in_beta, in_alpha]);
    let first = &hits[0];
    assert_eq!(first["sender"], "ayse");
    assert_eq!(first["sender_type"], "user");
    assert_eq!(first["text"], "a needle in beta too");
    assert!(first["ts"].as_u64().unwrap() > 0);
    let body = hits.to_string();
    assert!(
        !body.contains("gamma"),
        "a loca the caller lacks is never read"
    );
    assert!(
        !body.contains("private needle") && !body.contains(&dm),
        "someone else's direct message is never read"
    );

    // The direct message's own two people do find it.
    let (_, mehmets) = search(&base, &mehmet, "q=needle").await;
    let mehmet_rooms = rooms_of(&mehmets);
    assert!(mehmet_rooms.contains(&dm), "{mehmets}");
    assert!(mehmet_rooms.contains(&"gamma".to_string()));
    assert!(!mehmet_rooms.contains(&"alpha".to_string()));
    assert_eq!(mehmets[0]["id"].as_u64(), Some(in_dm));

    // The Master reaches every loca by rank, never a direct message.
    let r = reqwest::Client::new()
        .get(format!("{base}/search?q=needle"))
        .header("x-admin-token", MASTER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let masters: Value = r.json().await.unwrap();
    let master_rooms = rooms_of(&masters);
    for room in ["alpha", "beta", "gamma"] {
        assert!(master_rooms.contains(&room.to_string()), "{masters}");
    }
    assert!(!master_rooms.contains(&dm), "a rank never reads a DM");

    // An invite opens one loca: a session taken with it searches that one.
    let invite = invite_for(&base, MASTER, "beta", "visitor").await;
    let visitor = session(&base, &invite, "visitor", "agent").await;
    let (status, visits) = search(&base, &visitor, "q=needle").await;
    assert_eq!(status, 200, "{visits}");
    assert_eq!(rooms_of(&visits), ["beta"], "{visits}");

    // Bounded: a limit is honoured, and the snippet is cut around the match.
    let (_, one) = search(&base, &ayse, "q=needle&limit=1").await;
    assert_eq!(rooms_of(&one), ["beta"]);
    let long = format!("{} needle-far {}", "a".repeat(900), "b".repeat(900));
    say(&base, "alpha", &ayse, &long).await;
    let (_, far) = search(&base, &ayse, "q=needle-far").await;
    let snippet = far[0]["text"].as_str().unwrap();
    assert!(snippet.contains("needle-far"), "{snippet}");
    assert!(
        snippet.chars().count() <= 242,
        "{}",
        snippet.chars().count()
    );

    // A search needs words; no credential is no search.
    assert_eq!(search(&base, &ayse, "q=%20").await.0, 400);
    assert_eq!(search(&base, &ayse, "").await.0, 400);
    let anonymous = reqwest::Client::new()
        .get(format!("{base}/search?q=needle"))
        .send()
        .await
        .unwrap();
    assert!(
        [401, 403].contains(&anonymous.status().as_u16()),
        "{}",
        anonymous.status()
    );
}
