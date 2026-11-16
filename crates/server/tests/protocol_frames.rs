//! Conformance: `docs/protocol-frames.json` names every WebSocket frame the
//! server can send, with its fields, and every frame a client may send
//! (docs/protocol.md). The Python kit and the web client test their handler
//! tables against the same file.
//!
//! How far this goes:
//! * `ServerFrame` / `ClientFrame` — by exhaustiveness. A new variant does not
//!   compile here until it has a sample below, and the sample's serialized tag
//!   and fields must match the list.
//! * Hand-built frames (`serde_json::json!({"t": …})` in the server) — by a
//!   source scan of `crates/server/src`. It sees only that idiom: a frame
//!   built with a non-literal tag, `format!`, or another serializer is
//!   invisible to it. Their fields are checked on the wire instead, by
//!   `wait_for` in `ws_roundtrip.rs`, for every frame a round trip observes.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use protocol::{ChatMode, ClientFrame, MessageReactionEvent, ModState, RoomSettings, ServerFrame};
use serde_json::{json, Value};

const NOW_MS: u64 = 1_763_100_000_000;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn frame_list() -> Value {
    let text = std::fs::read_to_string(repo_root().join("docs/protocol-frames.json"))
        .expect("docs/protocol-frames.json");
    serde_json::from_str(&text).expect("docs/protocol-frames.json is JSON")
}

fn de<T: serde::de::DeserializeOwned>(value: Value) -> T {
    serde_json::from_value(value).expect("sample deserializes")
}

fn message() -> Value {
    json!({
        "id": 7, "room": "proto-room", "sender": "proto-ayse", "sender_type": "user",
        "target": "proto-bot", "text": "hello", "kind": "announce", "reply_to": 6,
        "reply_to_sender": "proto-bot", "ts": NOW_MS,
        "attachments": [{"id": "a", "sha256": "a", "name": "n.txt", "mime": "text/plain", "size": 1}]
    })
}

fn settings() -> RoomSettings {
    RoomSettings {
        lead: Some("proto-bot".into()),
        extensions: Some(vec!["notes".into()]),
        title: Some("Proto loca".into()),
        description: Some("What this loca is for.".into()),
        seats: Some(5),
        icon: Some("rocket".into()),
        color: Some("sky".into()),
        labels: vec!["backend".into()],
        vault_rev: Some(2),
        ..RoomSettings::default()
    }
}

/// One table drives both the exhaustive match and the samples, so a new
/// variant cannot be matched without also being sampled.
macro_rules! server_frames {
    ($($variant:ident => $sample:expr,)*) => {
        fn variant_of(frame: &ServerFrame) -> &'static str {
            match frame {
                $(ServerFrame::$variant { .. } => stringify!($variant),)*
            }
        }
        fn samples() -> Vec<(&'static str, ServerFrame)> {
            vec![$((stringify!($variant), $sample),)*]
        }
    };
}

// Every optional field is populated so its key is serialized.
server_frames! {
    History => ServerFrame::History { messages: vec![de(message())] },
    Memory => ServerFrame::memory("proto-room", None),
    MemoryVersion => ServerFrame::MemoryVersion { room: "proto-room".into(), version: 3 },
    Msg => ServerFrame::Msg { message: de(message()) },
    Reaction => ServerFrame::Reaction {
        reaction: MessageReactionEvent {
            message_id: 7,
            emoji: "+1".into(),
            actors: vec!["proto-bot".into()],
            owner: "proto-ayse".into(),
            reactor: "proto-bot".into(),
            active: true,
            ts: NOW_MS,
        },
    },
    Caretaker => ServerFrame::Caretaker { message: de(message()) },
    Turn => ServerFrame::Turn { messages: vec![de(message())] },
    Members => ServerFrame::Members {
        members: vec![de(json!({"name": "proto-bot", "type": "agent", "role": "lead"}))],
    },
    Renamed => ServerFrame::Renamed { from: "old".into(), to: "new".into() },
    Relabelled => ServerFrame::Relabelled { to: "new".into(), ids: vec![7], reply_ids: vec![8] },
    Control => ServerFrame::Control { cmd: "stop".into() },
    Note => ServerFrame::Note {
        note: de(json!({"key": "k", "title": "t", "body": "b", "can_write": [],
            "updated_by": "proto-ayse", "updated_at": NOW_MS, "rev": 1})),
    },
    NoteWarn => ServerFrame::NoteWarn { key: "k".into(), by: "x".into(), can_write: vec![] },
    Mode => ServerFrame::Mode { mode: ChatMode::Free },
    Settings => ServerFrame::Settings { settings: settings() },
    Typing => ServerFrame::Typing { name: "proto-ayse".into(), on: true },
    Kicked => ServerFrame::Kicked { name: "proto-bot".into(), banned: false },
    Evicted => ServerFrame::Evicted { name: "proto-bot".into(), identity: "mb:x".into(), session: 2 },
    Mod => ServerFrame::Mod { state: ModState::default() },
    Journal => ServerFrame::Journal {
        entry: de(json!({"id": 1, "room": "proto-room", "by": "proto-bot",
            "by_type": "agent", "text": "done", "at": NOW_MS})),
    },
    Vault => ServerFrame::Vault {
        room: "proto-room".into(), rev: 2, updated_by: "proto-ayse".into(), updated_at: NOW_MS,
    },
    Task => ServerFrame::Task {
        task: de(json!({"id": 1, "room": "proto-room", "title": "t", "created_by": "proto-ayse",
            "status": "open", "created_at": NOW_MS})),
    },
    Goal => ServerFrame::Goal {
        goal: de(json!({"id": 1, "room": "proto-room", "outcome": "o", "created_by": "proto-ayse",
            "completion": "manual", "status": "active", "created_at": NOW_MS})),
    },
    Care => ServerFrame::Care {
        signal: de(json!({"id": "c1", "room": "proto-room", "reason": "manual",
            "subject": "s", "attempt": 1, "at": NOW_MS})),
    },
    Attention => ServerFrame::Attention {
        attention: de(json!({"id": "a1", "room": "proto-room", "reason": "manual", "subject": "s",
            "audience": {"kind": "lead"}, "created_by": "proto-ayse", "created_at": NOW_MS,
            "attempt": 1, "status": "open"})),
    },
    Wait => ServerFrame::Wait {
        waiter: "proto-bot".into(),
        wait: Some(de(json!({"room": "proto-room", "waiter": "proto-bot",
            "waiting_for": "proto-ayse", "reason": "r", "since": NOW_MS}))),
    },
}

fn names(object: &Value) -> BTreeSet<String> {
    object
        .as_object()
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default()
}

fn listed_fields(entry: &Value) -> BTreeSet<String> {
    names(&entry["fields"])
}

#[test]
fn every_server_frame_variant_is_listed_with_its_fields() {
    let list = frame_list();
    let frames = &list["server_frames"];
    let mut sampled = BTreeSet::new();
    for (declared, frame) in samples() {
        assert_eq!(
            variant_of(&frame),
            declared,
            "sample for {declared} builds another variant"
        );
        let wire = serde_json::to_value(&frame).unwrap();
        let tag = wire["t"].as_str().expect("tagged by t").to_string();
        let entry = &frames[&tag];
        assert!(
            entry.is_object(),
            "ServerFrame::{declared} serializes as t={tag:?}, which docs/protocol-frames.json does not list"
        );
        assert_eq!(
            entry["source"],
            format!("ServerFrame::{declared}"),
            "t={tag}"
        );
        let mut keys = names(&wire);
        keys.remove("t");
        assert_eq!(keys, listed_fields(entry), "fields of t={tag}");
        sampled.insert(format!("ServerFrame::{declared}"));
    }
    let listed: BTreeSet<String> = frames
        .as_object()
        .unwrap()
        .values()
        .filter_map(|entry| entry["source"].as_str())
        .filter(|source| source.starts_with("ServerFrame::"))
        .map(str::to_string)
        .collect();
    assert_eq!(listed, sampled, "listed ServerFrame variants vs the enum");
}

/// `"t": "<literal>"` inside non-comment lines of server source files that are
/// not test modules.
fn hand_built_frame_tags() -> BTreeSet<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name != "tests") {
                    walk(&path, out);
                }
            } else if path.extension().is_some_and(|ext| ext == "rs")
                && path.file_name().is_some_and(|name| name != "tests.rs")
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&repo_root().join("crates/server/src"), &mut files);
    let mut tags = BTreeSet::new();
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        for line in source.lines() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let mut rest = line;
            while let Some(at) = rest.find("\"t\"") {
                rest = &rest[at + 3..];
                let after = rest.trim_start();
                let Some(after) = after.strip_prefix(':') else {
                    continue;
                };
                let Some(value) = after.trim_start().strip_prefix('"') else {
                    continue;
                };
                if let Some(end) = value.find('"') {
                    tags.insert(value[..end].to_string());
                }
            }
        }
    }
    tags
}

#[test]
fn every_hand_built_server_frame_is_listed() {
    let list = frame_list();
    let listed: BTreeSet<String> = list["server_frames"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(_, entry)| entry["source"] == "json")
        .map(|(tag, _)| tag.clone())
        .collect();
    assert_eq!(
        hand_built_frame_tags(),
        listed,
        "json!({{\"t\": …}}) frames in crates/server/src vs docs/protocol-frames.json"
    );
}

#[test]
fn core_types_match_the_list() {
    let list = frame_list();
    let types = &list["types"];
    // The recorded writer is never read from the wire (a client cannot
    // claim it): set it as the server does, then serialise.
    let mut written = de::<protocol::Message>(message());
    assert_eq!(
        written.sender_principal, None,
        "sender_principal is never deserialised"
    );
    let claimed: protocol::Message = de(json!({
        "id": 8, "room": "proto-room", "sender": "proto-ayse", "sender_type": "user",
        "text": "claimed", "ts": NOW_MS, "sender_principal": "pr_claimed"
    }));
    assert_eq!(
        claimed.sender_principal, None,
        "a client cannot claim a writer"
    );
    written.sender_principal = Some("pr_proto".into());
    let message = serde_json::to_value(written).unwrap();
    let member = serde_json::to_value(de::<protocol::Member>(
        json!({"name": "proto-bot", "type": "agent", "role": "lead", "full_name": "Proto Bot", "icon": "leaf", "avatar_color": "orange", "avatar_shape": "hexagon", "principal_id": "pr_proto"}),
    ))
    .unwrap();
    for (name, value) in [
        ("Message", message),
        ("Member", member),
        ("RoomSettings", serde_json::to_value(settings()).unwrap()),
    ] {
        assert_eq!(
            names(&value),
            listed_fields(&types[name]),
            "fields of {name}"
        );
    }
}

macro_rules! client_frames {
    ($($variant:ident => $sample:expr,)*) => {
        fn client_variant_of(frame: &ClientFrame) -> &'static str {
            match frame {
                $(ClientFrame::$variant { .. } => stringify!($variant),)*
            }
        }
        fn client_samples() -> Vec<(&'static str, Value)> {
            vec![$((stringify!($variant), $sample),)*]
        }
    };
}

client_frames! {
    Send => json!({"t": "send", "text": "hi", "target": "proto-bot", "reply_to": 1, "op_id": "op-1"}),
    Control => json!({"t": "control", "cmd": "stop"}),
    Typing => json!({"t": "typing", "on": true}),
}

#[test]
fn every_client_frame_variant_is_listed_with_its_fields() {
    let list = frame_list();
    let frames = &list["client_frames"];
    let mut sampled = BTreeSet::new();
    for (declared, sample) in client_samples() {
        let frame: ClientFrame = de(sample.clone());
        assert_eq!(client_variant_of(&frame), declared);
        // No `..`: a new field on a client frame must be added here, and every
        // listed field must reach the struct (serde ignores unknown keys, so a
        // misspelt one would arrive as None).
        let populated = match frame {
            ClientFrame::Send {
                target,
                text,
                reply_to,
                op_id,
            } => target.is_some() && !text.is_empty() && reply_to.is_some() && op_id.is_some(),
            ClientFrame::Control { cmd } => !cmd.is_empty(),
            ClientFrame::Typing { on } => on,
        };
        assert!(
            populated,
            "every sampled field of {declared} must reach the struct"
        );
        let tag = sample["t"].as_str().unwrap();
        let entry = &frames[tag];
        assert_eq!(
            entry["source"],
            format!("ClientFrame::{declared}"),
            "t={tag}"
        );
        let mut keys = names(&sample);
        keys.remove("t");
        assert_eq!(keys, listed_fields(entry), "fields of client t={tag}");
        sampled.insert(tag.to_string());
    }
    assert_eq!(names(frames), sampled, "listed client frames vs the enum");
}

/// A frame a client already handles but the server does not send yet is
/// listed under `not_sent_yet`. The day the server emits it, this fails: move
/// it to `server_frames`, where the tests above then hold it.
#[test]
fn not_sent_yet_frames_are_not_emitted() {
    let list = frame_list();
    let pending: BTreeSet<String> = names(&list["not_sent_yet"])
        .into_iter()
        .filter(|tag| !tag.starts_with('$'))
        .collect();
    let mut emitted = hand_built_frame_tags();
    for (_, frame) in samples() {
        let wire = serde_json::to_value(&frame).unwrap();
        emitted.insert(wire["t"].as_str().unwrap().to_string());
    }
    let server = names(&list["server_frames"]);
    for tag in &pending {
        assert!(
            !emitted.contains(tag),
            "the server now emits t={tag}: move it from not_sent_yet to server_frames"
        );
        assert!(!server.contains(tag), "t={tag} is listed twice");
    }
}
