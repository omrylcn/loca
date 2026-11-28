//! Mentat rooms through the hub, on a test clock: a room is founded as one
//! and has no Operator, it may wait for its mentat, the line calls requests
//! in the order they came, the room's mode lets in only the mentat and the
//! askers inside (no rank writes past it), every limit is enforced by the
//! server, a pause freezes the line, and the mentat runs it.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::{
    ChatMode, MentatAnswered, MentatEndReason, MentatLimits, MentatRequestStatus, MentatState,
    NewMentatRoom, PostMessage, RoomSettings, SenderType, SetMentatRoom,
};

use super::super::{
    BuildingRole, FoundMentatError, Hub, HubConfig, IdentityEdit, MentatError, PostReject,
    RequestAuthority,
};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::Store;

const T0: u64 = 1_800_000_000_000;
const ROOM: &str = "sage-room";

fn hub() -> Hub {
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).unwrap()),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    hub
}

fn at(ms: u64) {
    TEST_NOW.store(T0 + ms, Ordering::Relaxed);
}

fn say(hub: &Hub, who: &str, text: &str) -> Result<(), PostReject> {
    hub.post(
        ROOM,
        PostMessage {
            kind: Default::default(),
            sender: who.into(),
            sender_type: SenderType::Agent,
            target: None,
            text: text.into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        false,
        &who.into(),
        None,
    )
    .map(|_| ())
}

fn allow(hub: &Hub) -> Vec<String> {
    match hub.mode(ROOM) {
        ChatMode::Restricted { allow } => allow,
        other => panic!("expected the line's restricted mode, got {other:?}"),
    }
}

fn lines(hub: &Hub) -> Vec<String> {
    hub.recent_messages(ROOM)
        .into_iter()
        .filter(|m| m.sender == "loca")
        .map(|m| m.text)
        .collect()
}

fn master(hub: &Hub) -> RequestAuthority {
    hub.resolve_authority(Some("MASTER"), None)
}

fn smaster(principal: &str) -> RequestAuthority {
    RequestAuthority {
        principal_id: Some(principal.into()),
        display_name: Some(principal.into()),
        kind: Some(SenderType::User),
        building_role: Some(BuildingRole::Smaster),
    }
}

const TOPIC: &str = "sessions, keys and the release order";

fn found_as(hub: &Hub, room: &str, by: &RequestAuthority, mentat: Option<&str>) -> MentatState {
    hub.found_mentat_room(
        room,
        by,
        None,
        &IdentityEdit::default(),
        &NewMentatRoom {
            topic: TOPIC.into(),
            mentat: mentat.map(str::to_string),
        },
    )
    .expect("a mentat room is founded")
}

/// The room every test here works in, with `sage` as its mentat.
fn found(hub: &Hub) -> MentatState {
    found_as(hub, ROOM, &master(hub), Some("sage"))
}

fn set(mentat: Option<&str>, limits: Option<MentatLimits>) -> SetMentatRoom {
    SetMentatRoom {
        mentat: mentat.map(str::to_string),
        topic: None,
        limits,
    }
}

fn limits(questions: u32) -> MentatLimits {
    MentatLimits {
        questions,
        ..MentatLimits::default()
    }
}

fn current(hub: &Hub) -> protocol::MentatRequest {
    hub.mentat_state(ROOM)
        .unwrap()
        .current
        .expect("a consultation is inside")
}

#[test]
fn a_mentat_room_is_founded_as_one_with_no_operator_and_may_wait_for_its_mentat() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    let state = found_as(&hub, ROOM, &master(&hub), None);
    assert_eq!(state.mentat.topic, TOPIC);
    assert_eq!(state.mentat.mentat, None);
    assert_eq!(state.mentat.limits, MentatLimits::default());
    assert!(hub.is_mentat_room(ROOM));
    assert!(hub.loca_operator(ROOM).is_none(), "no Operator seat");
    // It is born with the Wiki and the Vault, and nobody may write yet.
    let settings = hub.settings(ROOM);
    assert!(settings.mentat_room);
    assert_eq!(settings.mentat, None);
    assert_eq!(
        settings.extensions,
        Some(vec!["wiki".to_string(), "vault".to_string()])
    );
    assert!(allow(&hub).is_empty());

    // The first rule: nobody asks a room that has no mentat.
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", "how do sessions renew?"),
        Err(MentatError::NoMentat)
    );

    // Appointing one opens the line and tells the room.
    let state = hub
        .set_mentat_room(ROOM, &set(Some("sage"), None), "master")
        .unwrap();
    assert_eq!(state.mentat.mentat.as_deref(), Some("sage"));
    assert_eq!(hub.settings(ROOM).mentat.as_deref(), Some("sage"));
    assert_eq!(allow(&hub), vec!["sage"]);
    assert!(lines(&hub)
        .iter()
        .any(|l| l == "master appointed @sage this room's mentat."));
    assert!(hub
        .open_mentat_request(ROOM, "ada", "how do sessions renew?")
        .is_ok());

    // An ordinary loca never becomes a mentat room.
    hub.create_loca("plain", &master(&hub), None).unwrap();
    assert_eq!(
        hub.set_mentat_room("plain", &set(Some("other"), None), "master"),
        Err(MentatError::NotMentatRoom)
    );
    assert!(!hub.is_mentat_room("plain"));
    // And the name of a mentat room is taken like any loca's.
    assert!(matches!(
        hub.found_mentat_room(
            ROOM,
            &master(&hub),
            None,
            &IdentityEdit::default(),
            &NewMentatRoom {
                topic: TOPIC.into(),
                mentat: None
            }
        ),
        Err(FoundMentatError::Loca(_))
    ));
}

#[test]
fn the_line_calls_in_order_and_no_rank_writes_past_it() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    assert_eq!(allow(&hub), vec!["sage"]);

    // The first request is called at once; the second waits.
    let ada = hub
        .open_mentat_request(ROOM, "ada", "how do sessions renew?")
        .unwrap();
    assert_eq!(ada.status, MentatRequestStatus::Called);
    let bob = hub
        .open_mentat_request(ROOM, "bob", "the release order")
        .unwrap();
    assert_eq!(bob.status, MentatRequestStatus::Waiting);
    assert_eq!(allow(&hub), vec!["sage", "ada"]);
    assert!(lines(&hub)
        .iter()
        .any(|l| l.starts_with("@ada: your turn with the mentat, sage (request #1)")));

    // Only the askers inside and the mentat may write; one who waits may not.
    assert_eq!(say(&hub, "bob", "me too?"), Err(PostReject::MentatLine));
    assert_eq!(say(&hub, "ada", "how do sessions renew?"), Ok(()));
    assert_eq!(say(&hub, "sage", "with the davet"), Ok(()));
    assert_eq!(current(&hub).status, MentatRequestStatus::Active);

    // A rank does not write past the line either: it asks like anyone else.
    let as_rank = hub.post(
        ROOM,
        PostMessage {
            kind: Default::default(),
            sender: "master".into(),
            sender_type: SenderType::User,
            target: None,
            text: "let me add something".into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        &"master".into(),
        None,
    );
    assert_eq!(as_rank.map(|_| ()), Err(PostReject::MentatLine));

    // One open request each.
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", "one more thing"),
        Err(MentatError::AlreadyInLine)
    );
    assert_eq!(
        hub.open_mentat_request(ROOM, "sage", "asking myself"),
        Err(MentatError::IsMentat)
    );

    // The asker ends it, saying whether it was answered; the next is called.
    let ended = hub
        .end_mentat_consultation(ROOM, "ada", false, Some(MentatAnswered::Yes))
        .unwrap();
    assert_eq!(ended.end_reason, Some(MentatEndReason::Asker));
    assert_eq!(ended.answered, Some(MentatAnswered::Yes));
    assert_eq!(current(&hub).id, bob.id);
    assert_eq!(allow(&hub), vec!["sage", "bob"]);
    assert_eq!(say(&hub, "ada", "thanks"), Err(PostReject::MentatLine));
}

#[test]
fn messages_before_the_answer_are_one_question_and_the_last_answer_ends_it() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.set_mentat_room(ROOM, &set(None, Some(limits(2))), "master")
        .unwrap();
    hub.open_mentat_request(ROOM, "ada", "two questions only")
        .unwrap();

    // A question written in two messages is one question.
    assert_eq!(say(&hub, "ada", "first, part one"), Ok(()));
    assert_eq!(say(&hub, "ada", "and part two"), Ok(()));
    let inside = current(&hub);
    assert_eq!(inside.questions, 1);
    assert!(inside.awaiting);

    assert_eq!(say(&hub, "sage", "answer one"), Ok(()));
    assert!(!current(&hub).awaiting);
    assert_eq!(say(&hub, "ada", "second"), Ok(()));
    assert_eq!(current(&hub).questions, 2);

    // The answer to the last question ends it.
    assert_eq!(say(&hub, "sage", "answer two"), Ok(()));
    assert!(hub.mentat_state(ROOM).unwrap().current.is_none());
    assert_eq!(say(&hub, "ada", "third"), Err(PostReject::MentatLine));
    assert!(lines(&hub)
        .iter()
        .any(|l| l == "Consultation #1 ended: its questions were used up."));
}

#[test]
fn the_sweep_skips_ends_and_expires_on_the_server_clock() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.open_mentat_request(ROOM, "ada", "called and never came")
        .unwrap();
    hub.open_mentat_request(ROOM, "bob", "comes and goes silent")
        .unwrap();

    // A called request has two minutes to write.
    at(119_000);
    hub.expire_mentat();
    assert_eq!(current(&hub).id, 1);
    at(121_000);
    hub.expire_mentat();
    assert_eq!(current(&hub).id, 2, "ada was skipped, bob called");
    assert!(lines(&hub)
        .iter()
        .any(|l| l.starts_with("Request #1 was skipped")));

    // Silence ends a consultation.
    assert_eq!(say(&hub, "bob", "are you there?"), Ok(()));
    assert_eq!(say(&hub, "sage", "yes"), Ok(()));
    at(121_000 + 119_000);
    hub.expire_mentat();
    assert!(hub.mentat_state(ROOM).unwrap().current.is_some());
    at(121_000 + 121_000);
    hub.expire_mentat();
    assert!(hub.mentat_state(ROOM).unwrap().current.is_none());
    assert!(lines(&hub)
        .iter()
        .any(|l| l == "Consultation #2 ended: nobody wrote for too long."));

    // A request that waited an hour leaves the line.
    hub.set_mode(ROOM, ChatMode::Paused).unwrap();
    hub.set_mode(ROOM, ChatMode::Free).unwrap();
    let cem = hub
        .open_mentat_request(ROOM, "cem", "goes in at once")
        .unwrap();
    assert_eq!(cem.status, MentatRequestStatus::Called);
    let dan = hub
        .open_mentat_request(ROOM, "dan", "waits behind cem")
        .unwrap();
    assert_eq!(say(&hub, "cem", "a long one"), Ok(()));
    let opened = T0 + 242_000;
    TEST_NOW.store(opened + 3_601_000, Ordering::Relaxed);
    hub.expire_mentat();
    let state = hub.mentat_state(ROOM).unwrap();
    assert!(state.waiting.iter().all(|r| r.id != dan.id));
    assert!(lines(&hub)
        .iter()
        .any(|l| l.starts_with(&format!("Request #{} left the mentat line", dan.id))));
}

#[test]
fn a_reply_under_way_is_not_silence() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.open_mentat_request(ROOM, "ada", "a hard question")
        .unwrap();
    assert_eq!(
        say(&hub, "ada", "why does the gate take ten minutes?"),
        Ok(())
    );

    // The mentat thinks for four minutes, saying it is replying.
    hub.typing(ROOM, "sage", true);
    at(4 * 60_000);
    hub.expire_mentat();
    assert_eq!(
        current(&hub).status,
        MentatRequestStatus::Active,
        "a model that is replying has not gone silent"
    );
    assert_eq!(say(&hub, "sage", "because it builds four browsers"), Ok(()));

    // Once nobody is replying, silence counts again, from the last word.
    at(4 * 60_000 + 121_000);
    hub.expire_mentat();
    assert!(hub.mentat_state(ROOM).unwrap().current.is_none());
}

#[test]
fn time_running_out_lets_the_mentat_give_its_last_answer() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.open_mentat_request(ROOM, "ada", "asked near the end")
        .unwrap();
    assert_eq!(say(&hub, "ada", "first"), Ok(()));
    assert_eq!(say(&hub, "sage", "answer"), Ok(()));
    at(9 * 60_000 + 50_000);
    assert_eq!(say(&hub, "ada", "one last thing"), Ok(()));

    // Ten minutes are up while the mentat owes an answer: the consultation
    // is closing, not cut.
    at(10 * 60_000 + 1_000);
    hub.expire_mentat();
    let inside = current(&hub);
    assert!(inside.closing);
    assert_eq!(inside.end_reason, Some(MentatEndReason::Time));
    assert_eq!(allow(&hub), vec!["sage"], "the askers may not write now");
    assert_eq!(say(&hub, "ada", "and another"), Err(PostReject::MentatLine));

    // The mentat's next message is the last of it.
    assert_eq!(say(&hub, "sage", "the last answer"), Ok(()));
    assert!(hub.mentat_state(ROOM).unwrap().current.is_none());
    assert!(lines(&hub)
        .iter()
        .any(|l| l == "Consultation #1 ended: its time ran out."));

    // With no answer owed, time simply ends it; and a closing consultation
    // the mentat never answers ends after the closing time.
    hub.open_mentat_request(ROOM, "bob", "never answered at the end")
        .unwrap();
    let start = 10 * 60_000 + 1_000;
    at(start + 1_000);
    assert_eq!(say(&hub, "bob", "anyone?"), Ok(()));
    hub.typing(ROOM, "sage", true);
    at(start + 1_000 + 10 * 60_000 + 1_000);
    hub.expire_mentat();
    assert!(current(&hub).closing);
    at(start + 1_000 + 10 * 60_000 + 1_000 + 121_000);
    hub.expire_mentat();
    assert!(hub.mentat_state(ROOM).unwrap().current.is_none());
}

#[test]
fn a_pause_freezes_the_line_and_resuming_restarts_the_clock() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.open_mentat_request(ROOM, "ada", "paused before she came")
        .unwrap();
    assert_eq!(
        hub.set_mode(ROOM, ChatMode::Paused).unwrap(),
        ChatMode::Paused
    );
    assert!(hub.mentat_state(ROOM).unwrap().paused);
    // Nobody writes while the line is paused, the mentat included.
    assert_eq!(say(&hub, "sage", "still here"), Err(PostReject::MentatLine));
    at(10 * 60_000);
    hub.expire_mentat();
    assert_eq!(
        current(&hub).status,
        MentatRequestStatus::Called,
        "a paused line does not move"
    );
    // Asking for any other mode resumes the line, not that mode.
    let resumed = hub.set_mode(ROOM, ChatMode::Free).unwrap();
    assert_eq!(
        resumed,
        ChatMode::Restricted {
            allow: vec!["sage".into(), "ada".into()]
        }
    );
    assert_eq!(current(&hub).called_at, Some(T0 + 10 * 60_000));
    at(10 * 60_000 + 30_000);
    hub.expire_mentat();
    assert!(hub.mentat_state(ROOM).unwrap().current.is_some());
}

#[test]
fn a_mentat_that_does_not_answer_pauses_its_own_line() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    for (i, asker) in ["ada", "bob", "cem"].iter().enumerate() {
        at(i as u64 * 1_000);
        hub.open_mentat_request(ROOM, asker, "is anybody answering here?")
            .unwrap();
    }
    // Two consultations in a row end in silence with no word from the mentat.
    assert_eq!(say(&hub, "ada", "hello?"), Ok(()));
    at(125_000);
    hub.expire_mentat();
    assert_eq!(current(&hub).owners, vec!["bob".to_string()]);
    assert_eq!(say(&hub, "bob", "hello?"), Ok(()));
    at(250_000);
    hub.expire_mentat();

    // The line paused itself: cem is not called into a room nobody answers.
    let state = hub.mentat_state(ROOM).unwrap();
    assert!(state.paused);
    assert!(state.current.is_none());
    assert_eq!(state.waiting.len(), 1);
    assert!(lines(&hub)
        .iter()
        .any(|l| l.starts_with("The line is paused: the mentat did not answer 2")));
    // Resuming calls the next.
    hub.set_mode(ROOM, ChatMode::Free).unwrap();
    assert_eq!(current(&hub).owners, vec!["cem".to_string()]);
}

#[test]
fn askers_join_withdraw_and_are_merged_and_the_mentat_runs_the_line() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    hub.open_mentat_request(ROOM, "ada", "inside right now")
        .unwrap();
    let bob = hub
        .open_mentat_request(ROOM, "bob", "the release order")
        .unwrap();
    let cem = hub
        .open_mentat_request(ROOM, "cem", "also the release order")
        .unwrap();
    let dan = hub.join_mentat_request(ROOM, "dan", bob.id).unwrap();
    assert_eq!(dan.owners, vec!["bob".to_string(), "dan".to_string()]);
    assert_eq!(
        hub.join_mentat_request(ROOM, "eve", 1),
        Err(MentatError::NotWaiting),
        "the one inside cannot be joined"
    );

    // One asker leaves a shared request; it stays for the other.
    let left = hub.withdraw_mentat_request(ROOM, "dan", bob.id).unwrap();
    assert_eq!(left.owners, vec!["bob".to_string()]);
    assert_eq!(left.status, MentatRequestStatus::Waiting);
    assert_eq!(
        hub.withdraw_mentat_request(ROOM, "dan", bob.id),
        Err(MentatError::NotYours)
    );

    // Only the mentat (or a rank who manages the room) merges two requests.
    assert_eq!(
        hub.merge_mentat_requests(ROOM, cem.id, bob.id, "bob", false),
        Err(MentatError::NotYours)
    );
    assert_eq!(
        hub.merge_mentat_requests(ROOM, bob.id, bob.id, "sage", false),
        Err(MentatError::SameRequest)
    );
    let merged = hub
        .merge_mentat_requests(ROOM, cem.id, bob.id, "sage", false)
        .unwrap();
    assert_eq!(merged.owners, vec!["bob".to_string(), "cem".to_string()]);
    let state = hub.mentat_state(ROOM).unwrap();
    assert_eq!(state.waiting.len(), 1);
    assert!(lines(&hub)
        .iter()
        .any(|l| l == "Request #3 was joined to request #2: its askers go in together."));

    // A bystander cannot end the consultation; the mentat can.
    assert_eq!(
        hub.end_mentat_consultation(ROOM, "bob", false, None),
        Err(MentatError::NotYours)
    );
    let ended = hub
        .end_mentat_consultation(ROOM, "sage", false, Some(MentatAnswered::No))
        .unwrap();
    assert_eq!(ended.end_reason, Some(MentatEndReason::Mentat));
    assert_eq!(
        ended.answered, None,
        "only an asker says whether it was answered"
    );
    assert_eq!(allow(&hub), vec!["sage", "bob", "cem"]);

    // The last asker withdrawing takes the request out of the line.
    let eve = hub
        .open_mentat_request(ROOM, "eve", "changed my mind about this")
        .unwrap();
    let out = hub.withdraw_mentat_request(ROOM, "eve", eve.id).unwrap();
    assert_eq!(out.status, MentatRequestStatus::Withdrawn);
    assert!(hub.mentat_state(ROOM).unwrap().waiting.is_empty());
    // A rank who manages the room ends a consultation too.
    let by_rank = hub
        .end_mentat_consultation(ROOM, "master", true, None)
        .unwrap();
    assert_eq!(by_rank.end_reason, Some(MentatEndReason::Rank));
}

#[test]
fn topics_are_one_plain_line() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", "   "),
        Err(MentatError::EmptyTopic)
    );
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", "help"),
        Err(MentatError::TopicTooShort)
    );
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", &"x".repeat(201)),
        Err(MentatError::TopicTooLong)
    );
    assert_eq!(
        hub.open_mentat_request(ROOM, "ada", "hidden\u{200B}character in it"),
        Err(MentatError::InvalidTopic)
    );
    let kept = hub
        .open_mentat_request(ROOM, "ada", "  two\n lines   folded  ")
        .unwrap();
    assert_eq!(kept.topic, "two lines folded");
    // A topic never reaches a server line, so an @name in it wakes nobody.
    hub.open_mentat_request(ROOM, "bob", "ask @cem about the release")
        .unwrap();
    assert!(lines(&hub).iter().all(|l| !l.contains("@cem")));
    // The room's own topic follows the same rule.
    assert_eq!(
        hub.set_mentat_room(
            ROOM,
            &SetMentatRoom {
                mentat: None,
                topic: Some("short".into()),
                limits: None
            },
            "master"
        ),
        Err(MentatError::TopicTooShort)
    );
    assert_eq!(
        hub.set_mentat_room(
            ROOM,
            &set(
                None,
                Some(MentatLimits {
                    session_secs: 100_000,
                    ..MentatLimits::default()
                })
            ),
            "master"
        ),
        Err(MentatError::BadLimits)
    );
}

#[test]
fn one_room_each_and_taking_the_appointment_back_keeps_the_room() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    // An agent is the mentat of one room.
    assert!(matches!(
        hub.found_mentat_room(
            "second-room",
            &master(&hub),
            None,
            &IdentityEdit::default(),
            &NewMentatRoom {
                topic: TOPIC.into(),
                mentat: Some("sage".into())
            }
        ),
        Err(FoundMentatError::Mentat(MentatError::MentatElsewhere))
    ));
    assert!(!hub.is_mentat_room("second-room"), "nothing was founded");

    hub.open_mentat_request(ROOM, "ada", "inside when it changes")
        .unwrap();
    hub.open_mentat_request(ROOM, "bob", "waiting when it changes")
        .unwrap();
    assert_eq!(say(&hub, "ada", "a question"), Ok(()));

    // Taking the appointment back ends the consultation and keeps the line.
    let state = hub.clear_mentat(ROOM, "master").unwrap();
    assert_eq!(state.mentat.mentat, None);
    assert!(state.current.is_none());
    assert_eq!(state.waiting.len(), 1);
    assert!(hub.is_mentat_room(ROOM), "it stays a mentat room");
    assert!(allow(&hub).is_empty());
    assert_eq!(hub.settings(ROOM).mentat, None);
    assert!(hub.settings(ROOM).mentat_room);
    assert_eq!(
        hub.open_mentat_request(ROOM, "cem", "nobody to ask right now"),
        Err(MentatError::NoMentat)
    );
    assert_eq!(hub.clear_mentat(ROOM, "master"), Err(MentatError::NoMentat));

    // A new mentat takes the line over.
    let state = hub
        .set_mentat_room(ROOM, &set(Some("oracle"), None), "master")
        .unwrap();
    assert_eq!(state.current.unwrap().owners, vec!["bob".to_string()]);
    assert_eq!(allow(&hub), vec!["oracle", "bob"]);
    // And the old one is free for another room.
    found_as(&hub, "second-room", &master(&hub), Some("sage"));
}

#[test]
fn the_directory_lists_rooms_and_a_smaster_manages_only_its_own() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let hub = hub();
    at(0);
    found(&hub);
    let founder = smaster("sm-founder");
    found_as(&hub, "sm-room", &founder, None);
    hub.open_mentat_request(ROOM, "ada", "inside for the listing")
        .unwrap();
    hub.open_mentat_request(ROOM, "bob", "waiting for the listing")
        .unwrap();

    let list = hub.mentat_directory();
    assert_eq!(list.len(), 2);
    let sage = list.iter().find(|l| l.room == ROOM).unwrap();
    assert_eq!(sage.topic, TOPIC);
    assert_eq!(sage.mentat.as_deref(), Some("sage"));
    assert!(sage.busy);
    assert_eq!(sage.waiting, 1);
    assert!(!sage.online, "nobody is connected in a hub test");
    let waiting = list.iter().find(|l| l.room == "sm-room").unwrap();
    assert_eq!(waiting.mentat, None);

    // The Master manages every mentat room; a Smaster only the one it founded.
    assert!(hub.may_manage_mentat(ROOM, &master(&hub)));
    assert!(hub.may_manage_mentat("sm-room", &master(&hub)));
    assert!(hub.may_manage_mentat("sm-room", &founder));
    assert!(!hub.may_manage_mentat(ROOM, &founder));
    assert!(!hub.may_manage_mentat("sm-room", &smaster("sm-other")));
}
