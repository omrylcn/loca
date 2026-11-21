//! A Master Agent opens a loca on a floor the Master designated, enters only
//! the locas it opened, and sets or clears the lead there and nowhere else
//! (owner's decisions, 2026-11-18). Every act is recorded on its behalf.
use super::master_agent::{building, json, send, session, Building, OFF};
use super::*;

/// A Master Agent with the switch on and the Smaster's floor designated,
/// signed in with its key; returns its session and key.
async fn appointed(b: &Building, name: &str) -> (String, String) {
    assert_eq!(b.setting(true).await.0, 200);
    let (status, v) = b.appoint(name).await;
    assert_eq!(status, 201, "{v}");
    let key = v["code"].as_str().unwrap().to_string();
    let s = session(&b.base, ("x-admin-token", &key), name, "agent").await;
    (s, key)
}

async fn open(b: &Building, ma: &str, name: &str, group: Option<&str>) -> (u16, String) {
    let mut body = serde_json::json!({ "name": name });
    if let Some(group) = group {
        body["group"] = Value::String(group.to_string());
    }
    send(
        reqwest::Method::POST,
        b.url("/rooms"),
        Some(("x-session-token", ma)),
        Some(body),
    )
    .await
}

async fn floor_of_smaster(b: &Building, name: &str) -> String {
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/groups"),
        Some(("x-session-token", &b.smaster)),
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    json(&text)["id"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn a_master_agent_opens_a_loca_only_on_a_designated_floor() {
    let b = building().await;
    let (ma, _) = appointed(&b, "automaton").await;
    let other_floor = floor_of_smaster(&b, "elsewhere").await;

    let (status, text) = open(&b, &ma, "alpha", Some(&b.floor)).await;
    assert_eq!(status, 201, "{text}");
    let v = json(&text);
    assert_eq!(v["group"], b.floor.as_str());
    assert!(v["operator"].is_null(), "no Operator seat: {v}");

    // Not on the Ground floor, not on a floor it was not given, not on one
    // that does not exist: the same answer, so it learns nothing of floors.
    let refusals = [
        open(&b, &ma, "ground-two", None).await,
        open(&b, &ma, "beta", Some(&other_floor)).await,
        open(&b, &ma, "gamma", Some("grp_0000000000000000")).await,
    ];
    for (status, text) in &refusals {
        assert_eq!(*status, 403, "{text}");
    }
    assert_eq!(refusals[0].1, refusals[1].1);
    assert_eq!(refusals[1].1, refusals[2].1);

    // An archived designated floor takes no new loca.
    let (status, _) = send(
        reqwest::Method::PATCH,
        b.url(&format!("/groups/{}", b.floor)),
        Some(("x-session-token", &b.smaster)),
        Some(serde_json::json!({ "archived": true })),
    )
    .await;
    assert_eq!(status, 200);
    let (status, text) = open(&b, &ma, "delta", Some(&b.floor)).await;
    assert_eq!(status, 403, "{text}");
    assert_eq!(text, refusals[0].1);

    // The floor's record says who opened it, on whose behalf.
    let (status, text) = send(
        reqwest::Method::GET,
        b.url(&format!("/groups/{}/log", b.floor)),
        Some(("x-session-token", &b.master)),
        None,
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let log = json(&text);
    let joined = log
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["action"] == "loca_joined" && e["subject"] == "alpha")
        .unwrap_or_else(|| panic!("{log}"));
    assert_eq!(joined["by"], "automaton");
    assert_eq!(joined["by_role"], "master_agent");
    assert_eq!(joined["on_behalf_of"]["master_agent"], "automaton");
    assert_eq!(joined["on_behalf_of"]["appointed_by"], "Master");
    assert!(joined["on_behalf_of"]["appointed_at"].as_u64().unwrap() > 0);
}

#[tokio::test]
async fn a_master_agent_enters_only_the_locas_it_opened() {
    let b = building().await;
    let (ma, key) = appointed(&b, "automaton").await;
    assert_eq!(open(&b, &ma, "alpha", Some(&b.floor)).await.0, 201);

    // Its own loca: listed, read, written, watched.
    for header in [
        ("x-session-token", ma.as_str()),
        ("x-admin-token", key.as_str()),
    ] {
        let (status, text) = send(reqwest::Method::GET, b.url("/rooms"), Some(header), None).await;
        assert_eq!(status, 200);
        let rooms: Vec<String> = json(&text)
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["room"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(rooms, vec!["alpha"], "{}", header.0);
        let (status, text) = send(
            reqwest::Method::GET,
            b.url("/rooms/alpha/messages"),
            Some(header),
            None,
        )
        .await;
        assert_eq!(status, 200, "{text}");
    }
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &ma)),
        Some(serde_json::json!({ "text": "project opened", "sender": "automaton", "sender_type": "agent" })),
    )
    .await;
    assert_eq!(status, 201, "{text}");
    assert_eq!(b.ws_status("alpha", &ma).await, 101);

    // Another loca on the very floor it was given, a Ground-floor loca,
    // Care and a direct message stay closed at every door.
    for room in [
        b.floor_loca.as_str(),
        "ground",
        "lobby",
        "iye",
        b.dm.as_str(),
    ] {
        let (status, text) = send(
            reqwest::Method::GET,
            b.url(&format!("/rooms/{room}/messages")),
            Some(("x-session-token", &ma)),
            None,
        )
        .await;
        assert!(matches!(status, 401 | 403 | 404), "{room}: {status} {text}");
        assert_ne!(b.ws_status(room, &ma).await, 101, "{room}: socket");
    }
    let seen = send(
        reqwest::Method::GET,
        b.url(&format!("/rooms/{}/messages", b.floor_loca)),
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
    assert_eq!(seen, ghost, "a same-floor loca it did not open is hidden");

    // A second Master Agent does not enter the first one's loca.
    let (status, v) = b.appoint("second-automaton").await;
    assert_eq!(status, 201, "{v}");
    let second = session(
        &b.base,
        ("x-admin-token", v["code"].as_str().unwrap()),
        "second-automaton",
        "agent",
    )
    .await;
    let (status, _) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &second)),
        None,
    )
    .await;
    assert_eq!(status, 404, "another Master Agent's loca is hidden");

    // The floor's founder and the Master enter it as any floor loca.
    for who in [b.smaster.as_str(), b.master.as_str()] {
        let (status, _) = send(
            reqwest::Method::GET,
            b.url("/rooms/alpha/messages"),
            Some(("x-session-token", who)),
            None,
        )
        .await;
        assert_eq!(status, 200);
    }
}

#[tokio::test]
async fn a_master_agent_sets_the_lead_only_in_its_own_locas() {
    let b = building().await;
    let (ma, _) = appointed(&b, "automaton").await;
    assert_eq!(open(&b, &ma, "alpha", Some(&b.floor)).await.0, 201);

    let lead = |room: &str, lead: Value| {
        let url = b.url(&format!("/rooms/{room}/lead"));
        let ma = ma.clone();
        async move {
            send(
                reqwest::Method::POST,
                url,
                Some(("x-session-token", &ma)),
                Some(serde_json::json!({ "lead": lead })),
            )
            .await
        }
    };
    let (status, text) = lead("alpha", Value::String("envoy".into())).await;
    assert_eq!(status, 200, "{text}");
    assert_eq!(json(&text)["lead"], "envoy");
    let (status, text) = lead("alpha", Value::Null).await;
    assert_eq!(status, 200, "{text}");
    assert!(json(&text)["lead"].is_null());

    // The announcements say on whose behalf.
    let (_, text) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &b.master)),
        None,
    )
    .await;
    let lines: Vec<String> = json(&text)
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["sender"] == "loca")
        .map(|m| m["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        lines,
        vec![
            "automaton set @lead envoy on behalf of Master Agent automaton, appointed by Master.",
            "automaton set @lead none on behalf of Master Agent automaton, appointed by Master.",
        ]
    );

    // Refused in every loca it did not open.
    for room in [
        b.floor_loca.as_str(),
        "ground",
        "lobby",
        "iye",
        b.dm.as_str(),
    ] {
        let (status, text) = lead(room, Value::String("envoy".into())).await;
        assert!(matches!(status, 401 | 403 | 404), "{room}: {status} {text}");
    }
    let (_, text) = send(
        reqwest::Method::GET,
        b.url("/rooms/ground/settings"),
        Some(("x-session-token", &b.master)),
        None,
    )
    .await;
    assert!(json(&text)["lead"].is_null(), "the Ground loca has no lead");
}

#[tokio::test]
async fn switching_off_or_revoking_closes_its_door_and_its_socket() {
    let b = building().await;
    let (ma, _) = appointed(&b, "automaton").await;
    assert_eq!(open(&b, &ma, "alpha", Some(&b.floor)).await.0, 201);

    let mut request = format!(
        "ws://127.0.0.1:{}/ws?room=alpha&name=automaton&type=agent",
        b.port
    )
    .into_client_request()
    .unwrap();
    request.headers_mut().insert(
        "Sec-WebSocket-Protocol",
        HeaderValue::from_str(&format!("loca.v1, loca.session.{ma}")).unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(request).await.unwrap();

    assert_eq!(b.setting(false).await.0, 200);
    let closed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while let Some(frame) = ws.next().await {
            match frame {
                Ok(WsMessage::Close(_)) | Err(_) => return true,
                _ => {}
            }
        }
        true
    })
    .await
    .unwrap_or(false);
    assert!(closed, "the socket closes when the switch goes off");
    let (status, text) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &ma)),
        None,
    )
    .await;
    assert_eq!((status, text.as_str()), (403, OFF));

    // On again: the door opens again; revoked: shut for good.
    assert_eq!(b.setting(true).await.0, 200);
    assert_eq!(b.ws_status("alpha", &ma).await, 101);
    let (_, text) = b
        .as_master(reqwest::Method::GET, "/master-agents", None)
        .await;
    let id = json(&text)[0]["id"].as_str().unwrap().to_string();
    let (status, _) = b
        .as_master(
            reqwest::Method::DELETE,
            &format!("/master-agents/{id}"),
            None,
        )
        .await;
    assert_eq!(status, 200);
    assert_ne!(b.ws_status("alpha", &ma).await, 101);
    let (status, _) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &ma)),
        None,
    )
    .await;
    assert_eq!(status, 401);
}

#[tokio::test]
async fn a_floor_taken_off_the_list_keeps_its_locas_but_takes_no_new_one() {
    let b = building().await;
    let (ma, _) = appointed(&b, "automaton").await;
    assert_eq!(open(&b, &ma, "alpha", Some(&b.floor)).await.0, 201);
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "floors": [] })),
        )
        .await;
    assert_eq!(status, 200);

    let (status, text) = open(&b, &ma, "beta", Some(&b.floor)).await;
    assert_eq!(status, 403, "no new loca there: {text}");
    let (status, text) = send(
        reqwest::Method::GET,
        b.url("/rooms/alpha/messages"),
        Some(("x-session-token", &ma)),
        None,
    )
    .await;
    assert_eq!(status, 200, "its loca there is still reachable: {text}");
    let (status, text) = send(
        reqwest::Method::POST,
        b.url("/rooms/alpha/lead"),
        Some(("x-session-token", &ma)),
        Some(serde_json::json!({ "lead": "envoy" })),
    )
    .await;
    assert_eq!(status, 200, "{text}");
}

#[tokio::test]
async fn the_masters_limits_bound_how_many_locas_it_opens() {
    let b = building().await;
    let (ma, _) = appointed(&b, "automaton").await;
    let other = floor_of_smaster(&b, "second-project").await;
    let (status, text) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({
                "on": true,
                "floors": [b.floor, other],
                "limits": { "locas_per_floor": 2, "locas_per_hour": 3 },
            })),
        )
        .await;
    assert_eq!(status, 200, "{text}");

    assert_eq!(open(&b, &ma, "a-one", Some(&b.floor)).await.0, 201);
    assert_eq!(open(&b, &ma, "a-two", Some(&b.floor)).await.0, 201);
    let (status, text) = open(&b, &ma, "a-three", Some(&b.floor)).await;
    assert_eq!(status, 429, "the floor's limit: {text}");
    assert_eq!(open(&b, &ma, "b-one", Some(&other)).await.0, 201);
    let (status, text) = open(&b, &ma, "b-two", Some(&other)).await;
    assert_eq!(status, 429, "the hour's limit: {text}");

    // The Master raises the hour's limit: the next one opens.
    let (status, _) = b
        .as_master(
            reqwest::Method::PUT,
            "/building/settings/master-agent",
            Some(serde_json::json!({ "on": true, "limits": { "locas_per_hour": 10 } })),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(open(&b, &ma, "b-two", Some(&other)).await.0, 201);
}
