//! Wait and Reminder are core: a loca follows up on a declared wait, a wait
//! cycle and an enabled silence check whatever its extensions are. A
//! reminder about a goal or a task needs the extension that serves goals or
//! tasks, so nobody is woken into a 403. A direct message raises none.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::{
    CareReason, CreateGoal, CreateTask, GoalCompletion, PostMessage, ReminderRecipient,
    RoomSettings, RuntimeHealthUpdate, SenderType, SetWait,
};

use super::super::{Hub, HubConfig};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::Store;

/// A hub with every reminder rule on at one second, a healthy Lead `lead`
/// seated in `proj`, and `proj` set to `extensions`.
fn hub_with(extensions: &[&str], silence: bool) -> Hub {
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
        Arc::new(Store::open(None).expect("store")),
        RoomSettings {
            care_wait_secs: 1,
            care_goal_secs: 1,
            care_task_secs: 1,
            care_silence_secs: u32::from(silence),
            care_cooldown_secs: 0,
            care_max_attempts: 3,
            care_recipient: ReminderRecipient::Lead,
            ..RoomSettings::default()
        },
        1,
    );
    hub.now_ms = test_now_ms;
    TEST_NOW.store(1_000, Ordering::Relaxed);
    hub.set_lead("proj", Some("lead".into()), "operator")
        .expect("lead");
    assert!(hub.join("proj", "member:lead", "lead", SenderType::Agent, 1));
    healthy(&hub);
    let wanted: Vec<String> = extensions.iter().map(|id| id.to_string()).collect();
    hub.set_extensions("proj", &wanted, "operator", SenderType::User)
        .expect("extensions");
    hub
}

fn healthy(hub: &Hub) {
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
    .expect("runtime health");
}

fn wait(hub: &Hub, by: &str, waiting_for: &str) {
    hub.set_wait(
        "proj",
        SetWait {
            by: by.into(),
            waiting_for: waiting_for.into(),
            reason: "needs the schema".into(),
        },
    )
    .expect("wait");
}

fn say(hub: &Hub, text: &str) {
    hub.post(
        "proj",
        PostMessage {
            kind: Default::default(),
            sender: "operator".into(),
            sender_type: SenderType::User,
            target: None,
            text: text.into(),
            reply_to: None,
            op_id: None,
            attachments: Vec::new(),
        },
        true,
        &"operator".into(),
        None,
    )
    .expect("message");
}

/// Every reminder reason the loca raised so far, sweep after sweep.
fn reasons_after_sweeps(hub: &Hub, from: u64, sweeps: u64) -> Vec<CareReason> {
    for step in 0..sweeps {
        TEST_NOW.store(from + step * 2_000, Ordering::Relaxed);
        healthy(hub);
        hub.tick_care();
    }
    let mut reasons: Vec<CareReason> = hub
        .attentions("proj")
        .into_iter()
        .map(|attention| attention.reason)
        .collect();
    reasons.sort_by_key(|reason| format!("{reason:?}"));
    reasons.dedup();
    reasons
}

#[test]
fn an_overdue_wait_reminds_in_a_loca_without_focus() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hub = hub_with(&["notes", "journal"], false);
    wait(&hub, "worker", "reviewer");
    let reasons = reasons_after_sweeps(&hub, 3_001, 1);
    assert_eq!(reasons, vec![CareReason::WaitOverdue], "{reasons:?}");
}

#[test]
fn a_wait_cycle_reminds_in_a_chat_only_loca() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hub = hub_with(&[], false);
    wait(&hub, "ada", "bob");
    wait(&hub, "bob", "ada");
    let reasons = reasons_after_sweeps(&hub, 3_001, 1);
    assert_eq!(reasons, vec![CareReason::WaitCycle], "{reasons:?}");
}

#[test]
fn the_silence_check_reminds_in_a_loca_without_focus() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hub = hub_with(&["notes"], true);
    say(&hub, "start the silence clock");
    let reasons = reasons_after_sweeps(&hub, 5_001, 1);
    assert_eq!(reasons, vec![CareReason::RoomSilence], "{reasons:?}");
}

/// Goals and tasks kept while Focus is off are not served, so they remind
/// nobody: only the wait does.
#[test]
fn kept_goals_and_tasks_do_not_remind_while_their_extension_is_off() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let hub = hub_with(&["notes", "focus"], false);
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship the note".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .expect("goal");
    hub.create_task(
        "proj",
        CreateTask {
            title: "write it".into(),
            from_message: None,
            assigned_to: Some("lead".into()),
            by: "operator".into(),
        },
        None,
    )
    .expect("task");
    hub.set_extensions("proj", &["notes".into()], "operator", SenderType::User)
        .expect("focus off");
    wait(&hub, "worker", "reviewer");
    let reasons = reasons_after_sweeps(&hub, 3_001, 8);
    assert_eq!(reasons, vec![CareReason::WaitOverdue], "{reasons:?}");
}

/// A stored goal and a stored, assigned task in `proj`.
fn goal_and_task(hub: &Hub) {
    hub.create_goal(
        "proj",
        CreateGoal {
            outcome: "ship the note".into(),
            checkpoint: None,
            stale_after_secs: None,
            completion: GoalCompletion::Manual,
            task_ids: Vec::new(),
            by: "operator".into(),
        },
    )
    .expect("goal");
    hub.create_task(
        "proj",
        CreateTask {
            title: "write it".into(),
            from_message: None,
            assigned_to: Some("lead".into()),
            by: "operator".into(),
        },
        None,
    )
    .expect("task");
}

/// Goal reminders follow Focus, which serves goals; task reminders follow
/// the Loca Board, which serves tasks. Each needs only its own extension.
#[test]
fn goal_reminders_follow_focus_and_task_reminders_follow_the_board() {
    let _clock = TEST_CLOCK_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for (extensions, goal, task) in [
        (&["focus", "board"][..], true, true),
        (&["focus"][..], true, false),
        (&["board"][..], false, true),
    ] {
        let hub = hub_with(&["focus", "board"], false);
        goal_and_task(&hub);
        let wanted: Vec<String> = extensions.iter().map(|id| id.to_string()).collect();
        hub.set_extensions("proj", &wanted, "operator", SenderType::User)
            .expect("extensions");
        let reasons = reasons_after_sweeps(&hub, 3_001, 8);
        assert_eq!(
            reasons.contains(&CareReason::GoalReminder),
            goal,
            "{extensions:?}: {reasons:?}"
        );
        assert_eq!(
            reasons.contains(&CareReason::TaskReminder),
            task,
            "{extensions:?}: {reasons:?}"
        );
    }
}
