use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use protocol::{
    AttentionAudience, AttentionStatus, CareReason, CreateAttention, CreateGoal, CreateTask,
    GoalCompletion, GoalStatus, PostMessage, ReminderRecipient, RoomSettings, RuntimeHealthUpdate,
    SenderType, ServerFrame, SetWait, TaskStatus, UpdateGoal, UpdateTask,
};

use super::{AttentionError, CareDraft, GoalError, Hub, HubConfig};
use crate::store::{BuildingRole, Store};

static TEST_NOW: AtomicU64 = AtomicU64::new(0);
static TEST_CLOCK_LOCK: Mutex<()> = Mutex::new(());

fn test_now_ms() -> u64 {
    TEST_NOW.load(Ordering::Relaxed)
}

#[test]
fn authority_resolves_server_side_principals_and_revocation_takes_effect_live() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("authority.db");
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    store
        .ensure_master_principal("MASTER", "operator", 1)
        .expect("master principal");
    store
        .add_smaster("sm_alice", "alice", 2)
        .expect("smaster principal");
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store,
        RoomSettings::default(),
        1,
    );

    let master = hub.resolve_authority(Some("MASTER"), None);
    assert_eq!(master.building_role, Some(BuildingRole::Master));
    assert!(master.principal_id.is_some());
    assert!(master.is_master());

    let smaster = hub.resolve_authority(Some("sm_alice"), None);
    assert_eq!(smaster.building_role, Some(BuildingRole::Smaster));
    assert!(smaster.principal_id.is_some());
    assert!(smaster.is_building_admin());
    assert!(!smaster.is_master());

    let unknown = hub.resolve_authority(Some("sm_alice-but-client-claims-master"), None);
    assert_eq!(unknown.building_role, None);
    assert!(!unknown.is_building_admin());

    hub.revoke_smaster("sm_alice").expect("revoke smaster");
    let revoked = hub.resolve_authority(Some("sm_alice"), None);
    assert_eq!(revoked.building_role, None);
    assert!(!revoked.is_building_admin());
}

#[test]
fn a_reply_to_a_caretaker_is_addressed_once() {
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("addressing.db");
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-dev".into()]),
        },
        store,
        RoomSettings::default(),
        1,
    );

    let base = protocol::Message {
        id: 5,
        room: "reviewer".into(),
        sender: "someone".into(),
        sender_type: SenderType::Agent,
        target: None,
        text: "answering".into(),
        kind: protocol::MessageKind::Say,
        reply_to: Some(1),
        reply_to_sender: Some("loca-dev".into()),
        ts: 1,
        attachments: Vec::new(),
    };

    // A reply whose author is a caretaker addresses them — the new path, with no
    // explicit target and no @mention.
    assert_eq!(
        hub.addressed_caretakers(&base),
        vec!["loca-dev".to_string()]
    );

    // When the same caretaker is ALSO the target and @mentioned, sort+dedup keep
    // exactly one summon (never three).
    let triple = protocol::Message {
        target: Some("loca-dev".into()),
        text: "@loca-dev answering".into(),
        ..base.clone()
    };
    assert_eq!(
        hub.addressed_caretakers(&triple),
        vec!["loca-dev".to_string()]
    );

    // A non-caretaker reply author addresses nobody.
    let non_caretaker = protocol::Message {
        reply_to_sender: Some("someone-else".into()),
        ..base.clone()
    };
    assert!(hub.addressed_caretakers(&non_caretaker).is_empty());
}

#[test]
fn reminder_state_exposes_progress_retry_and_caretaker_fallback() {
    assert_eq!(
        Hub::reminder_state(Some("lead"), 1, false),
        protocol::ReminderState::Running
    );
    assert_eq!(
        Hub::reminder_state(Some("lead"), 2, false),
        protocol::ReminderState::Overdue
    );
    assert_eq!(
        Hub::reminder_state(Some("loca-care"), 1, false),
        protocol::ReminderState::Stalled
    );
}

#[test]
fn stalled_goal_falls_back_to_caretaker_without_guessing_at_replacement() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-care".into()]),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("iye", "member:loca-care", "loca-care", SenderType::Agent, 1,));
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let health = |wake: &str| RuntimeHealthUpdate {
        wake: wake.into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", health("FAILED"))
        .expect("failed lead health");
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("caretaker health");
    let (mut events, _) = hub.subscribe("proj");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "release is verified".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing stalled goal escalation: {error}"),
        }
    };
    assert_eq!(signal.reason, CareReason::GoalReminder);
    assert_eq!(signal.owner.as_deref(), Some("loca-care"));
    assert_eq!(signal.state, protocol::ReminderState::Stalled);
    assert_eq!(
        signal.subject,
        "Goal: release is verified · Lead unavailable · loca-care holding continuity"
    );
    assert_eq!(
        hub.goals("proj")
            .into_iter()
            .find(|candidate| candidate.id == goal.id)
            .expect("goal retained")
            .status,
        GoalStatus::Active,
        "caretaker escalation must not complete or cancel the goal"
    );

    while events.try_recv().is_ok() {}
    hub.report_runtime_health("lead", health("IDLE"))
        .expect("recovered lead health");
    TEST_NOW.store(3_002, Ordering::Relaxed);
    hub.tick_care();
    let recovered = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing recovered lead reminder: {error}"),
        }
    };
    assert_eq!(recovered.owner.as_deref(), Some("lead"));
    assert_ne!(recovered.state, protocol::ReminderState::Stalled);
    assert_eq!(recovered.subject, "Goal: release is verified");
}

#[test]
fn stalled_goal_language_waits_for_bounded_escalation() {
    let mut first = "Goal: release is verified".to_string();
    Hub::goal_caretaker_subject(&mut first, false);
    assert_eq!(
        first,
        "Goal: release is verified · Lead unavailable · loca-care holding continuity"
    );

    let mut exhausted = "Goal: release is verified".to_string();
    Hub::goal_caretaker_subject(&mut exhausted, true);
    assert_eq!(
        exhausted,
        "Goal: release is verified · Lead remains unavailable · operator review needed"
    );
}

#[test]
fn long_lived_bearer_tokens_are_full_width_csprng_values() {
    let tokens: HashSet<String> = (0..1_000).map(|_| Hub::secure_token("dv_", 32)).collect();
    assert_eq!(
        tokens.len(),
        1_000,
        "generated bearer tokens must be unique"
    );
    assert!(tokens.iter().all(|token| {
        token.len() == 67
            && token.starts_with("dv_")
            && token[3..].bytes().all(|byte| byte.is_ascii_hexdigit())
    }));
}

#[test]
fn an_active_goal_requires_a_room_lead() {
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    let result = hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship safely".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    );
    assert!(matches!(result, Err(GoalError::LeadRequired)));

    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "ship safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal with lead");
    hub.update_goal(
        "proj",
        goal.id,
        UpdateGoal {
            outcome: None,
            checkpoint: None,
            stale_after_secs: None,
            completion: None,
            task_ids: None,
            status: Some(GoalStatus::Cancelled),
            by: "operator".into(),
        },
    )
    .expect("cancel");
    hub.set_lead("proj", None, "operator").expect("clear lead");
    assert!(matches!(
        hub.update_goal(
            "proj",
            goal.id,
            UpdateGoal {
                outcome: None,
                checkpoint: None,
                stale_after_secs: None,
                completion: None,
                task_ids: None,
                status: Some(GoalStatus::Active),
                by: "operator".into(),
            },
        ),
        Err(GoalError::LeadRequired)
    ));
}

#[test]
fn unused_admin_pairing_expires_after_five_minutes() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(10_000, Ordering::Relaxed);
    let (code, expires_at) = hub
        .rotate_admin_pairing_for(Hub::ADMIN_SESSION_TTL_MS)
        .expect("pairing");
    assert_eq!(
        expires_at,
        10_000 + Hub::ADMIN_PAIRING_TTL_MS,
        "the code has its own short deadline, independent of session lifetime"
    );
    TEST_NOW.store(expires_at, Ordering::Relaxed);
    assert_eq!(
        hub.consume_admin_pairing(&code),
        None,
        "an unused code is rejected at its deadline"
    );
}

#[test]
fn goal_care_ages_from_explicit_progress_not_unrelated_chat() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
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
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_goal_secs: 10,
            care_task_secs: 0,
            care_silence_secs: 0,
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    let task = hub
        .create_task(
            "proj",
            CreateTask {
                title: "ship the release".into(),
                from_message: None,
                assigned_to: Some("lead".into()),
                by: "operator".into(),
            },
        )
        .expect("task");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "public release is live".into(),
                checkpoint: Some("review receipt".into()),
                stale_after_secs: None,
                completion: GoalCompletion::AllTasks,
                task_ids: vec![task.id],
                by: "operator".into(),
            },
        )
        .expect("goal");
    assert_eq!(goal.progress_at, 1_000);

    // Room conversation is context, not evidence that the goal advanced.
    TEST_NOW.store(9_000, Ordering::Relaxed);
    hub.post(
        "proj",
        PostMessage {
            kind: Default::default(),
            sender: "operator".into(),
            sender_type: SenderType::User,
            target: None,
            text: "unrelated chat".into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        "operator",
    )
    .expect("chat");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(11_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("goal care was reset by unrelated chat: {error}"),
        }
    };
    assert_eq!(first.reason, protocol::CareReason::GoalReminder);
    assert_eq!(first.subject, "Goal: public release is live");
    assert_eq!(first.state, protocol::ReminderState::Running);
    TEST_NOW.store(12_002, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let retry = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("goal care retry missing: {error}"),
        }
    };
    assert_ne!(first.id, retry.id, "each delivery attempt has its own id");
    assert_eq!(retry.state, protocol::ReminderState::Overdue);
    assert_eq!(
        first.attention_id, retry.attention_id,
        "retries must remain one durable attention"
    );
    assert_eq!(hub.attentions("proj").len(), 1);

    // A real linked-task transition is goal progress and restarts its age.
    TEST_NOW.store(13_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Taken),
            assigned_to: None,
            by: "lead".into(),
        },
        false,
    )
    .expect("task progress");
    let progressed_goal_at = hub.goals("proj")[0].progress_at;
    assert!(progressed_goal_at >= 13_000);
    TEST_NOW.store(14_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Taken),
            assigned_to: None,
            by: "lead".into(),
        },
        false,
    )
    .expect("idempotent task patch");
    assert_eq!(
        hub.tasks("proj")[0].progress_at,
        13_000,
        "a no-op task patch must not manufacture progress"
    );
    assert_eq!(
        hub.goals("proj")[0].progress_at,
        progressed_goal_at,
        "a no-op task patch must not postpone goal care"
    );
    while events.try_recv().is_ok() {}

    TEST_NOW.store(progressed_goal_at + 9_999, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    assert!(
        !matches!(events.try_recv(), Ok(ServerFrame::Care { .. })),
        "goal reminded before its progress interval elapsed"
    );
    TEST_NOW.store(progressed_goal_at + 10_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    assert!(matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));
    let attentions = hub.attentions("proj");
    assert_eq!(
        attentions.len(),
        2,
        "new explicit progress starts a new attention generation"
    );
    assert_eq!(
        attentions
            .iter()
            .filter(|attention| attention.status == AttentionStatus::Resolved)
            .count(),
        1,
        "explicit progress resolves the previous stalled condition"
    );

    // The loop remains active while even one linked task is incomplete, then
    // closes deterministically and leaves no orphan reminder behind.
    TEST_NOW.store(progressed_goal_at + 11_000, Ordering::Relaxed);
    hub.update_task(
        "proj",
        task.id,
        UpdateTask {
            status: Some(TaskStatus::Done),
            assigned_to: None,
            by: "lead".into(),
        },
        false,
    )
    .expect("finish linked task");
    assert_eq!(hub.goals("proj")[0].status, GoalStatus::Achieved);
    assert!(hub
        .attentions("proj")
        .iter()
        .all(|attention| attention.status == AttentionStatus::Resolved));
    while events.try_recv().is_ok() {}
    TEST_NOW.store(progressed_goal_at + 30_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(
        !matches!(events.try_recv(), Ok(ServerFrame::Care { .. })),
        "a terminal goal must not re-arm its reminder loop"
    );
}

#[test]
fn goal_completion_conditions_are_explicit_and_non_vacuous() {
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let manual = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "operator verifies the release".into(),
                checkpoint: Some("operator says GO".into()),
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("taskless manual goal");
    assert_eq!(manual.status, GoalStatus::Active);
    hub.update_goal(
        "proj",
        manual.id,
        UpdateGoal {
            outcome: None,
            checkpoint: None,
            stale_after_secs: None,
            completion: None,
            task_ids: None,
            status: Some(GoalStatus::Cancelled),
            by: "operator".into(),
        },
    )
    .expect("operator closes manual goal");
    assert!(matches!(
        hub.create_goal(
            "proj",
            CreateGoal {
                outcome: "empty task set cannot finish itself".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::AllTasks,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        ),
        Err(GoalError::InvalidTasks)
    ));
}

#[test]
fn operator_can_route_reminders_to_one_specific_healthy_person() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("person-reminder.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 0,
            care_recipient: ReminderRecipient::Person {
                name: "reviewer".into(),
            },
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("proj", "member:reviewer", "reviewer", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("lead health");
    hub.report_runtime_health("reviewer", heartbeat())
        .expect("reviewer health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "publish safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing person reminder: {error}"),
        }
    };
    assert_eq!(signal.owner.as_deref(), Some("reviewer"));
    assert_eq!(
        signal.audience,
        AttentionAudience::Person {
            name: "reviewer".into()
        }
    );
    assert_eq!(hub.pending_care("proj", "reviewer").len(), 1);
    assert!(hub.pending_care("proj", "lead").is_empty());

    let mut rooms = hub.rooms.lock().expect("rooms");
    let room = rooms.get_mut("proj").expect("room");
    room.settings.care_recipient = ReminderRecipient::All;
    let all_signal = hub.make_care_signal(
        "proj",
        room,
        CareDraft {
            attention_key: "goal:all:1".into(),
            owner: Some("lead".into()),
            owner_principal_id: None,
            group: None,
            reason: CareReason::GoalReminder,
            target: None,
            participants: Vec::new(),
            subject: "whole-loca reminder".into(),
            attempt: 1,
            at: 2_000,
            escalated: false,
        },
    );
    assert_eq!(
        all_signal.audience,
        AttentionAudience::Group {
            names: vec!["lead".into(), "reviewer".into()]
        }
    );
}

// ---- Everyone (durable multi-recipient) reminder fan-out ----

/// Build a hub whose new rooms default to an `Everyone` room-silence reminder,
/// and seat a canonical roster of `names` (each admitted — which mints a
/// principal — and davetted into `room`). Returns the hub with a controllable
/// clock already at `t=1_000`.
fn everyone_hub_from_store(store: Arc<Store>) -> Hub {
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
        store,
        RoomSettings {
            care_silence_secs: 1,
            care_cooldown_secs: 0,
            care_max_attempts: 3,
            care_recipient: ReminderRecipient::All,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub
}

fn everyone_hub(db: &std::path::Path, room: &str, names: &[&str]) -> Hub {
    let hub = everyone_hub_from_store(Arc::new(Store::open(db.to_str()).expect("sqlite store")));
    for name in names {
        let member = hub.admit_member(name, "agent", "operator").expect("admit");
        hub.invite_member_to_room(&member.token, room, "operator")
            .expect("invite");
    }
    hub
}

fn arm_silence(hub: &Hub, room: &str, sender: &str) {
    hub.post(
        room,
        PostMessage {
            kind: Default::default(),
            sender: sender.into(),
            sender_type: SenderType::Agent,
            target: None,
            text: "hi".into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        sender,
    )
    .expect("seed message");
}

fn silence_attentions(hub: &Hub, room: &str) -> Vec<(String, protocol::Attention)> {
    let rooms = hub.rooms.lock().expect("rooms");
    let r = rooms.get(room).expect("room");
    let mut out: Vec<(String, protocol::Attention)> = r
        .attentions
        .iter()
        .filter(|(id, _)| id.contains(":silence:"))
        .map(|(id, a)| (id.clone(), a.clone()))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[test]
fn everyone_reminder_fans_out_to_a_durable_attention_per_member() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("fanout.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    let silence = silence_attentions(&hub, "proj");
    assert_eq!(
        silence.len(),
        3,
        "one attention per roster member, not one lead"
    );
    let mut owners: Vec<String> = silence
        .iter()
        .filter_map(|(_, a)| a.owner.clone())
        .collect();
    owners.sort();
    assert_eq!(owners, vec!["alice", "bob", "carol"]);
    // Every per-member attention shares the silence generation prefix (so the
    // durable + in-memory prefix resolvers close them together) and carries the
    // group audience (so Chat renders a single @all).
    let mut groups = std::collections::HashSet::new();
    for (id, attention) in &silence {
        assert!(
            id.starts_with("attention:proj:silence:"),
            "shared generation prefix: {id}"
        );
        // Each per-member signal is addressed to ONE person (no group audience →
        // no N×N wake); the generation identity for the single @all lives in
        // `group`, shared across members and carrying no secret token.
        assert!(
            matches!(attention.audience, AttentionAudience::Person { .. }),
            "per-member Person audience, not a group broadcast"
        );
        let group = attention
            .group
            .clone()
            .expect("per-member attention carries a group id");
        assert!(
            !group.contains("mb_") && !group.contains("dv_"),
            "group id is secret-free"
        );
        groups.insert(group);
    }
    assert_eq!(groups.len(), 1, "all members share one generation group id");
}

#[test]
fn everyone_reminder_reaches_offline_members_via_the_durable_outbox() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    // Nobody joins — every member is offline. They must still each get a durable
    // care_outbox row so the reminder is delivered on their first reconnect.
    let hub = everyone_hub(
        &dir.path().join("offline.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    for name in ["alice", "bob", "carol"] {
        let pending = hub.store.pending_care("proj", name);
        assert_eq!(
            pending.len(),
            1,
            "{name} (offline) has one durable reminder queued"
        );
        assert_eq!(pending[0].reason, CareReason::RoomSilence);
    }
}

#[test]
fn everyone_reminder_is_all_or_nothing_when_a_principal_cannot_resolve() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("all-or-nothing.sqlite");
    let hub = everyone_hub(&db, "proj", &["alice", "bob", "carol"]);
    arm_silence(&hub, "proj", "alice");

    // Break bob's principal resolution while his davet + membership stay active:
    // drop the credential that binds his member record to a principal (targeted
    // via his principal's unique display name). The roster is now inconsistent.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "DELETE FROM credentials WHERE principal_id IN
                 (SELECT id FROM principals WHERE display_name = 'bob')",
            [],
        )
        .unwrap();
    }

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    // All or nothing: not one attention, not one outbox row, and the mark is
    // untouched so the scheduler retries — never a partial "Everyone".
    assert!(
        silence_attentions(&hub, "proj").is_empty(),
        "a single unresolvable principal aborts the whole fan-out"
    );
    for name in ["alice", "carol"] {
        assert!(
            hub.store.pending_care("proj", name).is_empty(),
            "{name} got no outbox row from the aborted generation"
        );
    }
    let rooms = hub.rooms.lock().expect("rooms");
    assert!(
        !rooms
            .get("proj")
            .expect("room")
            .care_marks
            .contains_key("silence"),
        "the care mark did not advance"
    );
}

#[test]
fn everyone_reminder_retry_re_delivers_without_duplicating() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("retry.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");

    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(silence_attentions(&hub, "proj").len(), 3);

    // A later sweep re-delivers to the SAME generation; the per-member attention
    // ids are stable, so no new attention is created.
    TEST_NOW.store(9_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(
        silence_attentions(&hub, "proj").len(),
        3,
        "retry re-delivers to the same generation, never duplicates"
    );
}

#[test]
fn a_room_message_resolves_every_everyone_attention_together() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("resolve.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert_eq!(silence_attentions(&hub, "proj").len(), 3);

    // A new room message breaks the silence: every per-member attention of that
    // generation resolves together (the prefix resolver), not just one.
    TEST_NOW.store(6_000, Ordering::Relaxed);
    arm_silence(&hub, "proj", "bob");
    let silence = silence_attentions(&hub, "proj");
    assert_eq!(silence.len(), 3);
    assert!(
        silence
            .iter()
            .all(|(_, a)| a.status == AttentionStatus::Resolved),
        "a room message resolves all Everyone attentions, not one"
    );
}

#[test]
fn everyone_roster_drops_a_revoked_member() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let hub = everyone_hub(
        &dir.path().join("revoke.sqlite"),
        "proj",
        &["alice", "bob", "carol"],
    );
    assert_eq!(
        hub.everyone_recipients("proj").expect("full roster").len(),
        3
    );

    let carol = hub.member_by_name("carol").expect("carol").token;
    hub.revoke_member(&carol).expect("revoke carol");

    let roster = hub
        .everyone_recipients("proj")
        .expect("roster after revoke");
    assert_eq!(
        roster.len(),
        2,
        "a revoked member leaves the Everyone roster"
    );
    assert!(roster.iter().all(|(_, name)| name != "carol"));
}

#[test]
fn everyone_reconnect_replay_drops_a_revoked_principals_row() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    // Everyone is offline, so the fan-out writes a durable outbox row per member.
    let hub = everyone_hub(
        &dir.path().join("revoke-replay.sqlite"),
        "proj",
        &["alice", "carol"],
    );
    arm_silence(&hub, "proj", "alice");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();

    // carol's canonical principal, from the pre-revoke roster.
    let carol_pid = hub
        .everyone_recipients("proj")
        .expect("roster")
        .into_iter()
        .find(|(_, name)| name == "carol")
        .expect("carol on roster")
        .0;

    // Before revoke: carol's durable row replays to her principal exactly once.
    assert_eq!(
        hub.pending_care_scoped("proj", "carol", Some(&carol_pid))
            .len(),
        1,
        "carol's enqueued reminder replays before revoke"
    );

    // Revoke carol's davet AFTER her row was already enqueued.
    hub.revoke_invites_for("proj", "carol")
        .expect("revoke carol's davet");

    // The principal-scoped replay re-checks the LIVE roster and drops the stale
    // row: a revoked principal receives zero on reconnect.
    assert!(
        hub.pending_care_scoped("proj", "carol", Some(&carol_pid))
            .is_empty(),
        "a revoked principal's enqueued reminder is dropped on reconnect"
    );
    // The durable receipt is retained UNACKNOWLEDGED (audit truth) — it is
    // filtered at delivery, never fake-ACK'd.
    assert_eq!(
        hub.store.pending_care("proj", "carol").len(),
        1,
        "the outbox row is retained unacknowledged, not fake-ACK'd"
    );
}

#[test]
fn everyone_treats_two_same_named_principals_as_distinct_recipients() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("same-name.sqlite");
    let hub = everyone_hub_from_store(Arc::new(Store::open(db.to_str()).expect("store")));
    // Two distinct members with distinct canonical principals, both davetted into
    // the same room.
    let dave = hub.admit_member("dave", "agent", "operator").expect("dave");
    let other = hub
        .admit_member("dave-two", "agent", "operator")
        .expect("other");
    hub.invite_member_to_room(&dave.token, "proj", "operator")
        .expect("invite dave");
    hub.invite_member_to_room(&other.token, "proj", "operator")
        .expect("invite other");
    // Rename the second member to the SAME display name — in the store and the
    // in-memory roster — so two distinct principals now share a display name.
    {
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        conn.execute(
            "UPDATE members SET name = 'dave' WHERE token = ?1",
            rusqlite::params![other.token],
        )
        .unwrap();
    }
    hub.members
        .lock()
        .expect("members")
        .get_mut(&other.token)
        .expect("other membership")
        .name = "dave".into();

    // Deduped by canonical principal, never by name → two recipients, both "dave".
    let roster = hub.everyone_recipients("proj").expect("roster");
    assert_eq!(roster.len(), 2, "two principals, same name, two recipients");
    assert!(roster.iter().all(|(_, name)| name == "dave"));
    let principals: HashSet<String> = roster.iter().map(|(pid, _)| pid.clone()).collect();
    assert_eq!(principals.len(), 2, "distinct canonical principals");

    arm_silence(&hub, "proj", "dave");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    let silence = silence_attentions(&hub, "proj");
    assert_eq!(
        silence.len(),
        2,
        "one attention per principal, not merged by name"
    );
    let ids: HashSet<String> = silence.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(ids.len(), 2, "attention ids differ by principal");
    assert!(silence
        .iter()
        .all(|(_, a)| a.owner.as_deref() == Some("dave")));

    // Each same-named principal has its OWN durable outbox row, and the ACK is
    // gated by the authenticated principal — never the shared display name.
    let pids: Vec<String> = roster.iter().map(|(pid, _)| pid.clone()).collect();
    let care_a = hub
        .pending_care_scoped("proj", "dave", Some(&pids[0]))
        .pop()
        .expect("principal a has its own row");
    let care_b = hub
        .pending_care_scoped("proj", "dave", Some(&pids[1]))
        .pop()
        .expect("principal b has its own row");
    let sig_a = care_a.id.clone();
    let sig_b = care_b.id.clone();
    assert_ne!(sig_a, sig_b, "each principal's delivery is distinct");
    assert!(
        matches!(
            hub.resolve_attention("proj", &care_a.attention_id, "dave", Some(&pids[1]), false),
            Err(AttentionError::Forbidden)
        ),
        "a same-named principal cannot resolve another principal's attention"
    );
    assert!(
        matches!(
            hub.resolve_attention("proj", &care_a.attention_id, "dave", None, false),
            Err(AttentionError::Forbidden)
        ),
        "a principal-bound attention cannot be resolved by name alone"
    );
    // The OTHER same-named principal cannot ACK this row.
    assert!(
        !hub.ack_care(&sig_a, "dave", Some(&pids[1]))
            .expect("cross-principal ack"),
        "a same-named principal cannot ACK another's delivery"
    );
    // A legacy/name-only ACK (no authenticated principal) cannot ACK an
    // Everyone per-member row either — the reminder demands the principal.
    assert!(
        !hub.ack_care(&sig_a, "dave", None).expect("name-only ack"),
        "a principal-bound row is never ACK'd by display name alone"
    );
    // Its own principal ACKs it; the other principal's delivery stays pending.
    assert!(
        hub.ack_care(&sig_a, "dave", Some(&pids[0]))
            .expect("own ack a"),
        "the owning principal ACKs its own delivery"
    );
    assert!(
        hub.pending_care_scoped("proj", "dave", Some(&pids[0]))
            .is_empty(),
        "principal a's row is now acknowledged"
    );
    assert_eq!(
        hub.pending_care_scoped("proj", "dave", Some(&pids[1]))
            .len(),
        1,
        "principal b's delivery is untouched by a's ACK"
    );
    assert!(
        hub.ack_care(&sig_b, "dave", Some(&pids[1]))
            .expect("own ack b"),
        "principal b independently ACKs its own delivery"
    );
    assert_eq!(
        hub.resolve_attention("proj", &care_a.attention_id, "dave", Some(&pids[0]), false)
            .expect("owner resolves its own attention")
            .status,
        AttentionStatus::Resolved
    );

    // Revoking one same-named member drops only its own delivery; the other stays.
    hub.revoke_member(&dave.token).expect("revoke dave");
    let after = hub
        .everyone_recipients("proj")
        .expect("roster after revoke");
    assert_eq!(
        after.len(),
        1,
        "revoking one same-named member leaves the other"
    );
}

#[test]
fn a_legacy_invite_binds_to_its_principal_and_enters_the_roster_once() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("legacy.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("store"));
    // First boot: admit erin (membership + principal), then seed a LEGACY davet
    // that predates the member link — empty `member`, carrying only her name.
    let erin = {
        let hub = everyone_hub_from_store(store.clone());
        let erin = hub.admit_member("erin", "agent", "operator").expect("erin");
        let legacy = protocol::Invite {
            token: "dv_legacy_erin".into(),
            room: "proj".into(),
            member: String::new(),
            name: "erin".into(),
            kind: "agent".into(),
            issued_at: 1_000,
            issued_by: "operator".into(),
        };
        hub.store
            .insert_invite(&legacy)
            .expect("seed legacy invite");
        erin
    };
    // Second boot: the load-time migration binds the legacy davet to erin's
    // membership by name, so she resolves to her real canonical principal —
    // exactly once, never a fabricated or duplicate identity.
    let hub = everyone_hub_from_store(store);
    let roster = hub.everyone_recipients("proj").expect("roster");
    assert_eq!(
        roster.len(),
        1,
        "the migrated legacy davet enters the roster once"
    );
    assert_eq!(roster[0].1, "erin");
    assert_eq!(
        roster[0].0,
        hub.store
            .principal_id_for_member_record(&erin.token)
            .expect("erin principal"),
        "bound to erin's real canonical principal"
    );
}

#[test]
fn replacing_wait_resolves_old_attention_and_starts_a_new_generation() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("wait-attention.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 1,
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.set_wait(
        "proj",
        SetWait {
            by: "worker".into(),
            waiting_for: "reviewer".into(),
            reason: "needs review".into(),
        },
    )
    .expect("first wait");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("wait attention missing: {error}"),
        }
    };
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    TEST_NOW.store(3_000, Ordering::Relaxed);
    hub.set_wait(
        "proj",
        SetWait {
            by: "worker".into(),
            waiting_for: "security".into(),
            reason: "needs security review".into(),
        },
    )
    .expect("replacement wait");
    let old = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first.attention_id)
        .expect("old attention retained as history");
    assert_eq!(old.status, AttentionStatus::Resolved);
    assert!(hub.pending_care("proj", "lead").is_empty());

    while events.try_recv().is_ok() {}
    TEST_NOW.store(4_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    hub.tick_care();
    let next = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("replacement wait attention missing: {error}"),
        }
    };
    assert_ne!(next.attention_id, first.attention_id);
}

#[test]
fn attention_defaults_to_lead_and_delivery_ack_does_not_resolve_it() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
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
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");

    let attention = hub
        .create_attention(
            "proj",
            CreateAttention {
                subject: "review the release gate".into(),
                audience: AttentionAudience::Lead,
                by: "operator".into(),
            },
        )
        .expect("attention");
    assert_eq!(attention.owner.as_deref(), Some("lead"));
    assert_eq!(attention.status, AttentionStatus::Open);
    let delivery_id = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal.id,
            Ok(_) => continue,
            Err(error) => panic!("attention delivery missing: {error}"),
        }
    };
    assert!(!hub
        .ack_care("unknown-delivery", "lead", None)
        .expect("unknown ack"));
    assert!(!hub
        .ack_care(&delivery_id, "intruder", None)
        .expect("wrong-owner ack"));

    TEST_NOW.store(2_000, Ordering::Relaxed);
    let claimed = hub
        .claim_attention("proj", &attention.id, "lead")
        .expect("claim");
    assert_eq!(claimed.status, AttentionStatus::Claimed);
    assert_eq!(claimed.claimed_by.as_deref(), Some("lead"));

    TEST_NOW.store(3_000, Ordering::Relaxed);
    assert!(hub
        .ack_care(&delivery_id, "lead", None)
        .expect("delivery ack"));
    assert!(!hub
        .ack_care(&delivery_id, "lead", None)
        .expect("duplicate delivery ack"));
    let after_ack = hub.attentions("proj").pop().expect("attention remains");
    assert_eq!(after_ack.delivered_at, Some(3_000));
    assert_eq!(
        after_ack.status,
        AttentionStatus::Claimed,
        "runtime delivery must not manufacture work completion"
    );

    TEST_NOW.store(4_000, Ordering::Relaxed);
    let resolved = hub
        .resolve_attention("proj", &attention.id, "lead", None, false)
        .expect("resolve");
    assert_eq!(resolved.status, AttentionStatus::Resolved);
    assert_eq!(resolved.resolved_at, Some(4_000));
}

#[test]
fn claimed_attention_keeps_one_owner_when_retry_fallback_changes() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("claimed-owner.sqlite");
    let mut hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::from(["loca-care".to_string()]),
        },
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 1,
            care_max_attempts: 2,
            care_goal_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut project_events, _) = hub.subscribe("proj");
    let (mut home_events, _) = hub.subscribe("iye");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    assert!(hub.join("iye", "member:loca-care", "loca-care", SenderType::Agent, 1));
    let health = |wake: &str| RuntimeHealthUpdate {
        wake: wake.into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", health("IDLE"))
        .expect("lead health");
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("caretaker health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "one owner".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while project_events.try_recv().is_ok() {}
    while home_events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first = loop {
        match project_events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing lead delivery: {error}"),
        }
    };
    assert_eq!(first.owner.as_deref(), Some("lead"));
    hub.claim_attention("proj", &first.attention_id, "lead")
        .expect("lead claim");
    hub.report_runtime_health("lead", health("FAILED"))
        .expect("lead failure health");
    while project_events.try_recv().is_ok() {}
    while home_events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 3_000, Ordering::Relaxed);
    hub.report_runtime_health("loca-care", health("IDLE"))
        .expect("fresh caretaker health");
    hub.tick_care();
    assert!(
        !matches!(home_events.try_recv(), Ok(ServerFrame::Care { .. })),
        "claimed work must not be reassigned to the fallback caretaker"
    );
    assert!(hub.pending_care("iye", "loca-care").is_empty());
    let claimed = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first.attention_id)
        .expect("claimed attention");
    assert_eq!(claimed.status, AttentionStatus::Claimed);
    assert_eq!(claimed.owner.as_deref(), Some("lead"));
    assert_eq!(claimed.claimed_by.as_deref(), Some("lead"));
}

#[test]
fn group_attention_selects_exactly_one_healthy_claimant() {
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock test lock");
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    let _ = hub.subscribe("proj");
    assert!(hub.join("proj", "member:healthy", "healthy", SenderType::Agent, 1));
    hub.report_runtime_health(
        "healthy",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("runtime health");
    hub.report_runtime_health(
        "awake-but-not-seated",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("unseated runtime health");
    let attention = hub
        .create_attention(
            "proj",
            CreateAttention {
                subject: "one reviewer must inspect this".into(),
                audience: AttentionAudience::Group {
                    names: vec![
                        "offline".into(),
                        "healthy".into(),
                        "healthy".into(),
                        "awake-but-not-seated".into(),
                    ],
                },
                by: "operator".into(),
            },
        )
        .expect("group attention");
    assert_eq!(attention.owner.as_deref(), Some("healthy"));
    assert_eq!(
        attention.participants,
        vec!["awake-but-not-seated", "healthy", "offline"]
    );
}

#[test]
fn same_millisecond_care_conditions_keep_distinct_delivery_receipts() {
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("care-delivery.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("sqlite store"));
    let hub = Hub::build(
        HubConfig {
            admin_token: "MASTER".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        store.clone(),
        RoomSettings::default(),
        1,
    );
    let _ = hub.subscribe("proj");
    let rooms = hub.rooms.lock().expect("rooms");
    let room = rooms.get("proj").expect("room");
    let signal = |attention_key: &str| {
        hub.make_care_signal(
            "proj",
            room,
            CareDraft {
                attention_key: attention_key.into(),
                owner: Some("lead".into()),
                owner_principal_id: None,
                group: None,
                reason: CareReason::WaitOverdue,
                target: None,
                participants: Vec::new(),
                subject: attention_key.into(),
                attempt: 1,
                at: 1_000,
                escalated: false,
            },
        )
    };
    let first = signal("wait:a:1");
    let second = signal("wait:b:1");
    drop(rooms);

    assert_ne!(first.id, second.id);
    store.enqueue_care("proj", &first).expect("first enqueue");
    store.enqueue_care("proj", &second).expect("second enqueue");
    assert_eq!(
        store.pending_care("proj", "lead").len(),
        2,
        "neither delivery may disappear behind an outbox primary-key collision"
    );
}

#[test]
fn wait_cycle_survives_missing_owner_and_uses_bounded_scheduler() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("cycle-scheduler.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 600,
            care_cooldown_secs: 10,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    for (waiter, waiting_for) in [("a", "b"), ("b", "a")] {
        hub.set_wait(
            "proj",
            SetWait {
                by: waiter.into(),
                waiting_for: waiting_for.into(),
                reason: "cycle".into(),
            },
        )
        .expect("wait");
    }
    hub.tick_care();
    assert!(hub.attentions("proj").is_empty());

    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing cycle signal: {error}"),
        }
    };
    assert_eq!(first.reason, CareReason::WaitCycle);
    assert_eq!(first.attempt, 1);

    // Reposting the exact edge is a no-op and cannot manufacture a wake.
    hub.set_wait(
        "proj",
        SetWait {
            by: "b".into(),
            waiting_for: "a".into(),
            reason: "cycle".into(),
        },
    )
    .expect("idempotent wait");
    TEST_NOW.store(5_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    TEST_NOW.store(12_000, Ordering::Relaxed);
    hub.tick_care();
    let escalation = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing bounded escalation: {error}"),
        }
    };
    assert_eq!(escalation.attempt, 2);
    assert!(escalation.escalated);
    TEST_NOW.store(13_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));
}

#[test]
fn accepted_chat_retires_old_silence_attention_and_starts_a_new_generation() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("silence-generation.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            care_silence_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = || RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat())
        .expect("runtime health");
    let first_message = hub
        .post(
            "proj",
            PostMessage {
                kind: Default::default(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "first".into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            "operator",
        )
        .expect("first message");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(first_message.ts + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing first silence signal: {error}"),
        }
    };
    assert_eq!(first_signal.reason, CareReason::RoomSilence);
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    TEST_NOW.store(first_message.ts + 2_000, Ordering::Relaxed);
    let second_message = hub
        .post(
            "proj",
            PostMessage {
                kind: Default::default(),
                sender: "operator".into(),
                sender_type: SenderType::User,
                target: None,
                text: "progress".into(),
                reply_to: None,
                op_id: None,
                attachments: Vec::new(),
            },
            true,
            "operator",
        )
        .expect("progress message");
    let retired = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first_signal.attention_id)
        .expect("old silence attention retained as history");
    assert_eq!(retired.status, AttentionStatus::Resolved);
    assert!(hub.pending_care("proj", "lead").is_empty());

    while events.try_recv().is_ok() {}
    TEST_NOW.store(second_message.ts + 1_001, Ordering::Relaxed);
    hub.report_runtime_health("lead", heartbeat())
        .expect("refreshed runtime health");
    hub.tick_care();
    let next_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing next silence generation: {error}"),
        }
    };
    assert_eq!(next_signal.reason, CareReason::RoomSilence);
    assert_ne!(next_signal.attention_id, first_signal.attention_id);
}

#[test]
fn archived_room_pauses_care_and_unarchive_resumes_same_open_attention() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("archive-resume.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            care_goal_secs: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    let heartbeat = RuntimeHealthUpdate {
        wake: "IDLE".into(),
        ack: "OK".into(),
        delivery_id: None,
        attention_id: None,
        stored: false,
        accepted: false,
        first_response: false,
        final_response: false,
        turn_completed: false,
    };
    hub.report_runtime_health("lead", heartbeat.clone())
        .expect("runtime health");
    let goal = hub
        .create_goal(
            "proj",
            CreateGoal {
                outcome: "ship safely".into(),
                checkpoint: None,
                stale_after_secs: None,
                completion: GoalCompletion::Manual,
                task_ids: Vec::new(),
                by: "operator".into(),
            },
        )
        .expect("goal");
    while events.try_recv().is_ok() {}

    TEST_NOW.store(goal.progress_at + 1_001, Ordering::Relaxed);
    hub.tick_care();
    let first_signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing goal reminder: {error}"),
        }
    };
    assert_eq!(hub.pending_care("proj", "lead").len(), 1);

    hub.set_settings(
        "proj",
        None,
        None,
        None,
        Some(true),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("archive");
    let paused = hub
        .attentions("proj")
        .into_iter()
        .find(|attention| attention.id == first_signal.attention_id)
        .expect("open attention remains visible while archived");
    assert_eq!(paused.status, AttentionStatus::Open);
    assert!(hub.pending_care("proj", "lead").is_empty());
    while events.try_recv().is_ok() {}
    TEST_NOW.store(goal.progress_at + 2_000, Ordering::Relaxed);
    hub.tick_care();
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    hub.set_settings(
        "proj",
        None,
        None,
        None,
        Some(false),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .expect("unarchive");
    while events.try_recv().is_ok() {}
    hub.report_runtime_health("lead", heartbeat)
        .expect("refreshed runtime health");
    hub.tick_care();
    let resumed = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("missing resumed goal reminder: {error}"),
        }
    };
    assert_eq!(resumed.attention_id, first_signal.attention_id);
    assert_ne!(resumed.id, first_signal.id);
}

#[test]
fn runtime_readiness_expires_without_a_renewed_native_lease() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
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
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.report_runtime_health(
        "native-lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "IDLE".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("initial native lease");
    assert!(
        hub.runtime_health_for("native-lead")
            .expect("reported runtime")
            .ready
    );

    TEST_NOW.store(21_001, Ordering::Relaxed);
    assert!(
        !hub.runtime_health_for("native-lead")
            .expect("expired runtime remains observable")
            .ready,
        "a stopped Monitor must become ineligible without a shutdown report"
    );
}

#[test]
fn disjoint_wait_cycle_is_not_starved_by_resolved_first_cycle() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
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
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings {
            care_wait_secs: 600,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    for (waiter, waiting_for) in [("a", "b"), ("b", "a"), ("c", "d"), ("d", "c")] {
        hub.set_wait(
            "proj",
            SetWait {
                by: waiter.into(),
                waiting_for: waiting_for.into(),
                reason: "cycle".into(),
            },
        )
        .expect("wait");
    }
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("first cycle missing: {error}"),
        }
    };
    assert_eq!(first.participants, vec!["a", "b"]);
    hub.resolve_attention("proj", &first.attention_id, "lead", None, false)
        .expect("resolve first cycle");
    while events.try_recv().is_ok() {}
    TEST_NOW.store(2_000, Ordering::Relaxed);
    hub.tick_care();
    let second = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("second cycle starved: {error}"),
        }
    };
    assert_eq!(second.participants, vec!["c", "d"]);
}

#[test]
fn clearing_and_recreating_wait_in_same_millisecond_uses_new_generation() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("wait-generation.sqlite");
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
        Arc::new(Store::open(db.to_str()).expect("sqlite store")),
        RoomSettings {
            care_wait_secs: 0,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    let first_wait = hub
        .set_wait(
            "proj",
            SetWait {
                by: "worker".into(),
                waiting_for: "reviewer".into(),
                reason: "review".into(),
            },
        )
        .expect("wait");
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let first = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("first wait attention missing: {error}"),
        }
    };
    hub.clear_wait("proj", "worker").expect("clear");
    drop(events);
    drop(hub);
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
        Arc::new(Store::open(db.to_str()).expect("restart store")),
        RoomSettings {
            care_wait_secs: 0,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        2,
    );
    hub.now_ms = test_now_ms;
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 2));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("restart health");
    let recreated = hub
        .set_wait(
            "proj",
            SetWait {
                by: "worker".into(),
                waiting_for: "reviewer".into(),
                reason: "review".into(),
            },
        )
        .expect("recreate");
    assert!(recreated.since > first_wait.since);
    while events.try_recv().is_ok() {}
    hub.tick_care();
    let second = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("recreated wait was suppressed: {error}"),
        }
    };
    assert_ne!(second.attention_id, first.attention_id);
    assert_eq!(
        hub.attentions("proj")
            .iter()
            .filter(|attention| attention.status == AttentionStatus::Open)
            .count(),
        1
    );
}

#[test]
fn care_attempt_is_not_burned_when_attention_storage_fails() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(|lock| lock.into_inner());
    let dir = tempfile::tempdir().expect("temp store");
    let db = dir.path().join("care-atomic.sqlite");
    let store = Arc::new(Store::open(db.to_str()).expect("sqlite store"));
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
        store,
        RoomSettings {
            care_goal_secs: 1,
            care_cooldown_secs: 0,
            care_max_attempts: 1,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    let (mut events, _) = hub.subscribe("proj");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    hub.report_runtime_health(
        "lead",
        RuntimeHealthUpdate {
            wake: "IDLE".into(),
            ack: "OK".into(),
            delivery_id: None,
            attention_id: None,
            stored: false,
            accepted: false,
            first_response: false,
            final_response: false,
            turn_completed: false,
        },
    )
    .expect("health");
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .expect("goal");
    while events.try_recv().is_ok() {}

    let sql = rusqlite::Connection::open(&db).expect("probe db");
    sql.execute_batch(
        "CREATE TRIGGER reject_care_attention
         BEFORE INSERT ON attentions
         BEGIN SELECT RAISE(FAIL, 'injected care failure'); END;",
    )
    .expect("trigger");
    TEST_NOW.store(2_001, Ordering::Relaxed);
    hub.tick_care();
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM care_marks),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("counts");
    assert_eq!(counts, (0, 0, 0));
    assert!(!matches!(events.try_recv(), Ok(ServerFrame::Care { .. })));

    sql.execute_batch("DROP TRIGGER reject_care_attention")
        .expect("drop trigger");
    TEST_NOW.store(2_002, Ordering::Relaxed);
    hub.tick_care();
    let signal = loop {
        match events.try_recv() {
            Ok(ServerFrame::Care { signal }) => break signal,
            Ok(_) => continue,
            Err(error) => panic!("care did not recover: {error}"),
        }
    };
    assert_eq!(signal.attempt, 1);
    let counts: (u64, u64, u64) = sql
        .query_row(
            "SELECT
                (SELECT count(*) FROM care_marks),
                (SELECT count(*) FROM attentions),
                (SELECT count(*) FROM care_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("counts");
    assert_eq!(counts, (1, 1, 1));
}

#[test]
fn join_request_rate_limit_is_per_source_not_global() {
    use super::JoinRequestCreate;
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock lock");
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
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);

    let a: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let b: std::net::IpAddr = "10.0.0.2".parse().unwrap();
    let cap = Hub::JOIN_CREATE_MAX_IN_WINDOW;

    // Source A exhausts its own window with distinct names (no NameTaken noise).
    for i in 0..cap {
        assert!(
            matches!(
                hub.create_join_request(&format!("a{i}"), "agent", a),
                JoinRequestCreate::Created { .. }
            ),
            "A request {i} should be admitted within the window"
        );
    }
    // One more from A is refused — the per-source window is full.
    assert!(matches!(
        hub.create_join_request("a-over", "agent", a),
        JoinRequestCreate::BacklogFull
    ));
    // A GLOBAL limiter would also refuse B here (the counter is already at cap).
    // Per-source isolation means B's own window is untouched -> Created.
    assert!(matches!(
        hub.create_join_request("b0", "agent", b),
        JoinRequestCreate::Created { .. }
    ));

    // Sliding window: once A's timestamps age past the window, A is admitted again.
    TEST_NOW.store(1_000 + Hub::JOIN_CREATE_WINDOW_MS + 1, Ordering::Relaxed);
    assert!(matches!(
        hub.create_join_request("a-later", "agent", a),
        JoinRequestCreate::Created { .. }
    ));
}

#[test]
fn approve_issued_membership_authenticates_on_a_persistent_store() {
    use super::{Approve, JoinRequestCreate};
    let _clock = TEST_CLOCK_LOCK.lock().expect("clock lock");
    let directory = tempfile::tempdir().expect("tempdir");
    let path = directory.path().join("lobby-auth.db");
    // A PERSISTENT (file-backed) store: the Lobby authenticates through the
    // credentials table here, NOT the in-memory cache (which only backs a
    // non-persistent store), so this is the configuration that caught the bug.
    let store = Arc::new(Store::open(Some(path.to_str().expect("db path"))).expect("store"));
    store
        .ensure_master_principal("MASTER", "operator", 1)
        .expect("master");
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
        store,
        RoomSettings::default(),
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);

    // Master mints one right; an outside agent requests to join.
    hub.mint_admission_stock(1, 3_600_000);
    let ip: std::net::IpAddr = "10.0.0.5".parse().unwrap();
    let JoinRequestCreate::Created {
        request_id,
        request_secret,
    } = hub.create_join_request("newbie", "agent", ip)
    else {
        panic!("expected Created");
    };

    // Master approves; the agent bootstraps its mb_ exactly once.
    assert!(matches!(
        hub.approve_join_request(&request_id, "operator"),
        Approve::Approved
    ));
    let mb = hub
        .claim_join_request_bootstrap(&request_id, &request_secret)
        .expect("bootstrap yields the mb_");
    assert!(mb.starts_with("mb_"));

    // REGRESSION GUARD (review blocker): the approve-issued mb_ must AUTHENTICATE
    // immediately at the Lobby entry point on a persistent store. `member_for_credential`
    // resolves through the credentials table here — RED before the fix (approve wrote
    // only the `members` row, so this returned None -> the joining agent got 401
    // until an unrelated server restart backfilled the credential).
    let member = hub
        .member_for_credential(Some(&mb))
        .expect("approve-issued mb_ must authenticate immediately on a persistent store");
    assert_eq!(member.name, "newbie");
    assert_eq!(member.kind, "agent");
}

#[test]
fn join_request_is_announced_visibly_in_the_home_loca_without_leaking_the_secret() {
    use super::JoinRequestCreate;
    let hub = Hub::build(
        HubConfig {
            admin_token: "M".into(),
            room_token: String::new(),
            require_sessions: false,
            require_invite: false,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(None).expect("memory store")),
        RoomSettings::default(),
        1,
    );
    // Deliberately do NOT join the home loca first: this covers the review
    // blocker where `iye` is not yet in memory. The announce must STILL surface
    // (create must never silently drop it), so create_join_request materialises
    // the home loca and posts regardless.
    let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
    let out = hub.create_join_request("visitor", "agent", ip);
    let JoinRequestCreate::Created { request_secret, .. } = out else {
        panic!("expected Created");
    };

    // A visible "<name> wants to join" announce must land in the home loca so the
    // Master sees it in the main app — the whole point of this fix. RED before it
    // (create only wrote the hidden request row). And it must carry NO secret.
    let (_rx, history) = hub.subscribe("iye");
    let announce = history
        .iter()
        .find(|m| m.sender == "loca" && m.text.contains("wants to join"))
        .unwrap_or_else(|| {
            panic!(
                "no visible join-request announce in the home loca; history: {:?}",
                history.iter().map(|m| &m.text).collect::<Vec<_>>()
            )
        });
    assert!(announce.text.contains("visitor"));
    assert!(
        !announce.text.contains(&request_secret) && !announce.text.contains("jrs_"),
        "the announce must never carry the request secret"
    );
}
