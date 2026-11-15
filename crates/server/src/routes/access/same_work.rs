//! A direct message or group loca the caller may not enter must cost the door the
//! same work as an id that names nothing, not only give the same answer
//! (`tests/ws/hidden_locas.rs` pins the answer). A direct message id is a hash of
//! two principal ids a Smaster can read from `GET /profiles`; if the hidden
//! case took one more SQLite read or one more middleware than the nothing
//! case, timing would tell the Smaster who talks to whom.
//!
//! Timing itself is too noisy for CI, so this counts the work instead, with
//! the test-only tally in `crate::sync::work`: every state mutex and every
//! SQLite connection the request takes on this (single, current-thread)
//! test thread. A REST request runs through the real middlewares in the
//! order `main.rs` layers them, into a `RoomAccess` handler. The WebSocket
//! door cannot be driven in process (the upgrade needs a real connection),
//! so its part that reads the loca id, `socket_sees_nothing` after the
//! session is resolved, is counted directly; `ws_handler` looks nothing up
//! about the id before it.
use super::*;
use crate::hub::HubConfig;
use crate::sync::work;
use std::sync::Arc;
use tower::ServiceExt;

const ROOT: &str = "same-work-root";
const MAIN_RS: &str = include_str!("../../main.rs");

type Credential = (&'static str, String);

struct Fixture {
    _dir: tempfile::TempDir,
    hub: Hub,
    dm: String,
    group_loca: String,
    dm_outsiders: Vec<(&'static str, Credential)>,
    group_outsiders: Vec<(&'static str, Credential)>,
}

fn body(name: &str) -> CreateSession {
    CreateSession {
        name: name.into(),
        kind: None,
        runtime: None,
        capabilities: vec![],
    }
}

/// `POST /sessions` with a Smaster code or a member's own key: no invite, so
/// the credential's principal names the session.
fn key_session(hub: &Hub, key: &str) -> String {
    hub.create_session_scoped(
        body("ignored"),
        None,
        hub.admin_session_grant(Some(key), Hub::ADMIN_SESSION_TTL_MS),
        hub.reaches_building(Some(key)),
        None,
    )
    .expect("session saved")
    .expect("session recorded")
    .session_token
}

/// `POST /sessions` with an invite: the session is confined to its loca.
fn invite_session(hub: &Hub, invite_key: &str) -> String {
    let invite = hub.invite_by_token(invite_key).expect("live invite");
    hub.create_session_scoped(body("ignored"), Some(&invite), None, false, None)
        .expect("session saved")
        .expect("session recorded")
        .session_token
}

/// The Building of `tests/ws/hidden_locas.rs`, built in process: Smasters
/// `deputy` (founds group `squad` and its loca `squad-atelier`) and `runnerup`;
/// members `opal` and `envoy` seated in `atelier`, `envoy` also in the group,
/// and `opal`'s direct message with `envoy`; `idler`, seated nowhere; the caretaker;
/// `lodger`, who holds only an invite for `atelier`.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = Arc::new(
        crate::store::Store::open(dir.path().join("same-work.db").to_str()).expect("store"),
    );
    let hub = Hub::build(
        HubConfig {
            admin_token: ROOT.into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: ["loca-care".to_string()].into_iter().collect(),
        },
        store,
        protocol::RoomSettings::default(),
        1,
    );
    let master = hub.resolve_authority(Some(ROOT), None);

    let deputy_key = hub.add_smaster("deputy").expect("smaster");
    let runnerup_key = hub.add_smaster("runnerup").expect("smaster");
    let deputy = key_session(&hub, &deputy_key);
    let runnerup = key_session(&hub, &runnerup_key);

    // Each member's key, handed over once at its admission, and its id.
    let mut keys = HashMap::new();
    let mut ids = HashMap::new();
    for (name, kind) in [
        ("opal", "user"),
        ("envoy", "agent"),
        ("idler", "user"),
        ("loca-care", "agent"),
        ("lodger", "agent"),
    ] {
        let (member, key) = hub.admit_member_once(name, kind, "master").expect("member");
        keys.insert(name, key.expect("a new member's key"));
        ids.insert(name, member.id);
    }
    hub.create_loca("atelier", &master, None).expect("atelier");
    let mut lodger_invite = String::new();
    for name in ["opal", "envoy", "lodger"] {
        let (_, invite) = hub
            .invite_member_to_room(&ids[name], "atelier", "master")
            .expect("seat in atelier");
        if name == "lodger" {
            lodger_invite = invite;
        }
    }
    hub.invite_member_to_room(&ids["loca-care"], "iye", "master")
        .expect("caretaker invite");

    let opal = key_session(&hub, &keys["opal"]);
    let idler = key_session(&hub, &keys["idler"]);
    let care = key_session(&hub, &keys["loca-care"]);
    let lodger = invite_session(&hub, &lodger_invite);

    let founder = hub.resolve_authority(None, Some(&deputy));
    let group = hub.create_group("squad", &founder).expect("group").id;
    let envoy_id = hub
        .resolve_authority(Some(&keys["envoy"]), None)
        .principal_id
        .expect("envoy principal");
    hub.add_group_member(&group, &envoy_id, &founder)
        .expect("group member");
    hub.create_loca_in("squad-atelier", &founder, None, Some(&group))
        .expect("group loca");

    let opener = hub.session_identity(Some(&opal)).expect("opal session");
    let dm = hub.open_dm(&opener, "envoy").expect("dm").0.room;

    let s = |token: &String| ("x-session-token", token.clone());
    let dm_outsiders = vec![
        ("smaster (session)", s(&deputy)),
        ("smaster (own key)", ("x-admin-token", deputy_key.clone())),
        ("other smaster", s(&runnerup)),
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
        ("other smaster (session)", s(&runnerup)),
        ("other smaster (own key)", ("x-admin-token", runnerup_key)),
        ("member outside the group", s(&opal)),
        ("lobby member", s(&idler)),
        ("caretaker", s(&care)),
        ("invite holder (session)", s(&lodger)),
        ("invite holder (invite)", ("x-room-token", lodger_invite)),
    ];
    Fixture {
        _dir: dir,
        hub,
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

/// The route layers `main.rs` puts on the router, outermost last.
fn main_layers() -> Vec<String> {
    MAIN_RS
        .split(".route_layer(axum::middleware::from_fn_with_state(")
        .skip(1)
        .map(|chunk| {
            chunk
                .split(',')
                .nth(1)
                .expect("a layer function")
                .trim()
                .split(')')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The router's gates as `main.rs` stacks them, around one `RoomAccess`
/// handler that says when a request got through.
fn gates(hub: &Hub) -> axum::Router {
    assert_eq!(
        main_layers(),
        [
            "extension_fence",
            "dm_fence",
            "group_fence",
            "require_membership"
        ],
        "main.rs changed its route layers; mirror them here"
    );
    axum::Router::new()
        .route(
            "/rooms/:id/messages",
            axum::routing::get(|_: RoomAccess| async { "reached" }),
        )
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            crate::routes::extension_fence,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            crate::routes::dm_fence,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            crate::routes::group_fence,
        ))
        .route_layer(axum::middleware::from_fn_with_state(
            hub.clone(),
            require_membership,
        ))
        .with_state(hub.clone())
}

#[derive(Debug, PartialEq, Eq)]
struct Door {
    status: u16,
    body: String,
    /// `(state mutexes taken, SQLite connections taken)`.
    work: (u64, u64),
}

async fn get_messages(app: &axum::Router, loca: &str, credential: &Credential) -> Door {
    let request = axum::http::Request::builder()
        .uri(format!("/rooms/{loca}/messages"))
        .header(credential.0, &credential.1)
        .body(axum::body::Body::empty())
        .expect("request");
    work::take();
    let response = app.clone().oneshot(request).await.expect("infallible");
    let work = work::take();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    Door {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        work,
    }
}

async fn rest_differences(
    fixture: &Fixture,
    loca: &str,
    outsiders: &[(&'static str, Credential)],
    table: &mut Vec<String>,
) -> Vec<String> {
    let app = gates(&fixture.hub);
    let mut differences = Vec::new();
    for (who, credential) in outsiders {
        // Settle once-only caches (a member's principal) before counting.
        get_messages(&app, loca, credential).await;
        let real = get_messages(&app, loca, credential).await;
        assert!(
            matches!(real.status, 401 | 403 | 404),
            "{who} on {loca} must be refused: {real:?}"
        );
        table.push(format!("{who:<32} {:<16} {:?}", short(loca), real.work));
        for nothing in nothing_ids(&fixture.dm) {
            let none = get_messages(&app, &nothing, credential).await;
            table.push(format!("{who:<32} {:<16} {:?}", short(&nothing), none.work));
            if real != none {
                differences.push(format!("{who}: real {real:?} / {nothing} {none:?}"));
            }
        }
    }
    differences
}

fn short(id: &str) -> String {
    if id.len() > 16 {
        format!("{}…", &id[..12])
    } else {
        id.to_string()
    }
}

#[tokio::test]
async fn a_hidden_loca_costs_the_rest_door_what_nothing_costs() {
    let fixture = fixture();
    let mut table = Vec::new();
    let dm = fixture.dm.clone();
    let group = fixture.group_loca.clone();
    let mut differences = rest_differences(&fixture, &dm, &fixture.dm_outsiders, &mut table).await;
    differences
        .extend(rest_differences(&fixture, &group, &fixture.group_outsiders, &mut table).await);
    eprintln!("caller / loca / (mutexes, sqlite):\n{}", table.join("\n"));
    assert!(
        differences.is_empty(),
        "the door works differently for a hidden loca than for nothing in {} cases:\n{}",
        differences.len(),
        differences.join("\n")
    );
}

/// One WebSocket handshake's work from the moment the door reads its
/// credentials: the session, then `socket_sees_nothing`, which `ws_handler`
/// asks before anything else reads the loca id. The credential rides as a
/// client sends it (`tests/ws/hidden_locas.rs::ws`).
fn socket_door(hub: &Hub, loca: &str, credential: &Credential) -> (bool, (u64, u64)) {
    let carried = |header: &str| (credential.0 == header).then_some(credential.1.as_str());
    work::take();
    let session = hub.session_identity(carried("x-session-token"));
    let refused = socket_sees_nothing(
        hub,
        loca,
        carried("x-admin-token"),
        carried("x-room-token"),
        session.as_ref(),
    );
    (refused, work::take())
}

#[test]
fn a_hidden_loca_costs_the_socket_door_what_nothing_costs() {
    let fixture = fixture();
    let mut table = Vec::new();
    let mut differences = Vec::new();
    for (loca, outsiders) in [
        (&fixture.dm, &fixture.dm_outsiders),
        (&fixture.group_loca, &fixture.group_outsiders),
    ] {
        for (who, credential) in outsiders {
            socket_door(&fixture.hub, loca, credential);
            let real = socket_door(&fixture.hub, loca, credential);
            assert!(real.0, "{who} must be refused at the socket door of {loca}");
            table.push(format!("{who:<32} {:<16} {:?}", short(loca), real.1));
            for nothing in nothing_ids(&fixture.dm) {
                let none = socket_door(&fixture.hub, &nothing, credential);
                table.push(format!("{who:<32} {:<16} {:?}", short(&nothing), none.1));
                if real != none {
                    differences.push(format!("{who}: real {real:?} / {nothing} {none:?}"));
                }
            }
        }
    }
    eprintln!("caller / loca / (mutexes, sqlite):\n{}", table.join("\n"));
    assert!(
        differences.is_empty(),
        "the socket door works differently for a hidden loca than for nothing in {} cases:\n{}",
        differences.len(),
        differences.join("\n")
    );
}
