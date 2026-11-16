//! A task given to someone: the one chat line that wakes them, and the
//! principal it belongs to (not whoever wears the name later).

use std::sync::Arc;

use protocol::{CreateTask, MessageKind, ServerFrame, TaskStatus, UpdateTask};

use super::super::{Hub, TaskError};
use super::{admit_with_key, critical_restart_hub};
use crate::store::Store;

fn declare(hub: &Hub, by: &str, assignee: Option<&str>) -> protocol::Task {
    hub.create_task(
        "proj",
        CreateTask {
            title: "fix @bystander's login".into(),
            from_message: None,
            assigned_to: assignee.map(str::to_string),
            by: by.into(),
        },
        None,
    )
    .expect("task")
}

fn patch(status: Option<TaskStatus>, assigned_to: Option<&str>, by: &str) -> UpdateTask {
    UpdateTask {
        status,
        assigned_to: assigned_to.map(str::to_string),
        by: by.into(),
    }
}

/// Every chat line the room was sent since the last call.
fn lines(events: &mut tokio::sync::broadcast::Receiver<ServerFrame>) -> Vec<protocol::Message> {
    let mut out = Vec::new();
    while let Ok(frame) = events.try_recv() {
        if let ServerFrame::Msg { message } = frame {
            out.push(message);
        }
    }
    out
}

#[test]
fn giving_a_task_to_someone_says_so_once_and_addresses_only_them() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("tasks.db").to_str()).unwrap());
    let hub = critical_restart_hub(store.clone(), "task-root");
    let first = declare(&hub, "operator", None);
    let (mut events, _) = hub.subscribe("proj");

    // Declared with an assignee: one announcement, to the assignee.
    let given = declare(&hub, "operator", Some("worker"));
    let said = lines(&mut events);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].kind, MessageKind::Announce);
    assert_eq!(said[0].sender, "loca");
    assert_eq!(said[0].target.as_deref(), Some("worker"));
    assert_eq!(
        said[0].text,
        format!("operator assigned task #{} to @worker.", given.id)
    );
    // The title (which names somebody else) is not repeated in the line, so
    // it cannot wake anybody but the assignee.
    assert!(!said[0].text.contains("bystander"));
    // It is part of the loca's record, not only a live frame.
    assert_eq!(
        store.message_owner("proj", said[0].id).unwrap().as_deref(),
        Some("loca")
    );

    // The same assignment again, a take and a finish: nobody is newly
    // assigned, so nothing is said.
    hub.update_task(
        "proj",
        given.id,
        patch(None, Some("worker"), "operator"),
        true,
        None,
    )
    .unwrap();
    hub.update_task(
        "proj",
        given.id,
        patch(Some(TaskStatus::Taken), None, "worker"),
        false,
        None,
    )
    .unwrap();
    assert!(lines(&mut events).is_empty());

    // The operator hands it to someone else: one line, to the new assignee.
    hub.update_task(
        "proj",
        given.id,
        patch(None, Some("other"), "operator"),
        true,
        None,
    )
    .unwrap();
    let said = lines(&mut events);
    assert_eq!(said.len(), 1, "{said:?}");
    assert_eq!(said[0].target.as_deref(), Some("other"));
    assert!(said[0].text.contains("@other"));

    // Taking an unassigned task, or declaring one for yourself, wakes nobody.
    hub.update_task(
        "proj",
        first.id,
        patch(Some(TaskStatus::Taken), Some("worker"), "worker"),
        false,
        None,
    )
    .unwrap();
    declare(&hub, "operator", Some("operator"));
    assert!(lines(&mut events).is_empty());
}

#[test]
fn a_task_belongs_to_the_principal_it_was_given_to_not_to_the_next_wearer_of_the_name() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("tasks.db").to_str()).unwrap());
    let hub = critical_restart_hub(store.clone(), "task-root");
    let (ada, _) = admit_with_key(&hub, "ada", "agent", "master");
    let ada_principal = ada.principal_id.clone().expect("a member has a principal");

    let task = declare(&hub, "operator", Some("ada"));
    assert_eq!(
        task.assigned_principal.as_deref(),
        Some(ada_principal.as_str())
    );

    // Ada leaves the Building; somebody else is admitted under her name.
    assert!(hub.revoke_member(&ada.id).unwrap());
    let (next, _) = admit_with_key(&hub, "ada", "agent", "master");
    let next_principal = next.principal_id.clone().expect("a member has a principal");
    assert_ne!(next_principal, ada_principal);

    // The new "ada" can neither take, finish nor re-claim Ada's task.
    for refused in [
        patch(Some(TaskStatus::Taken), None, "ada"),
        patch(Some(TaskStatus::Done), None, "ada"),
        patch(Some(TaskStatus::Taken), Some("ada"), "ada"),
    ] {
        assert!(matches!(
            hub.update_task("proj", task.id, refused, false, Some(&next_principal)),
            Err(TaskError::Forbidden)
        ));
    }
    let kept = &hub.tasks("proj")[0];
    assert_eq!(kept.status, TaskStatus::Open, "a refusal changes nothing");
    assert_eq!(
        kept.assigned_principal.as_deref(),
        Some(ada_principal.as_str())
    );
    assert_eq!(
        store.load_tasks("proj")[0].assigned_principal,
        kept.assigned_principal
    );

    // The principal it was given to still is its assignee.
    hub.update_task(
        "proj",
        task.id,
        patch(Some(TaskStatus::Taken), None, "ada"),
        false,
        Some(&ada_principal),
    )
    .unwrap();

    // The operator gives it to the new "ada": now it is theirs.
    hub.update_task(
        "proj",
        task.id,
        patch(None, Some("other"), "operator"),
        true,
        None,
    )
    .unwrap();
    hub.update_task(
        "proj",
        task.id,
        patch(None, Some("ada"), "operator"),
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        hub.tasks("proj")[0].assigned_principal.as_deref(),
        Some(next_principal.as_str())
    );
    hub.update_task(
        "proj",
        task.id,
        patch(Some(TaskStatus::Done), None, "ada"),
        false,
        Some(&next_principal),
    )
    .unwrap();
}

#[test]
fn a_task_given_by_name_alone_is_bound_to_whoever_it_names_when_they_take_it() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open(directory.path().join("tasks.db").to_str()).unwrap());
    let hub = critical_restart_hub(store.clone(), "task-root");
    // Nobody wears "late" yet: the task is a name alone, as in the sandbox and
    // on every row written before principals were recorded.
    let task = declare(&hub, "operator", Some("late"));
    assert_eq!(task.assigned_principal, None);
    let (late, _) = admit_with_key(&hub, "late", "agent", "master");
    let late_principal = late.principal_id.clone().unwrap();
    let taken = hub
        .update_task(
            "proj",
            task.id,
            patch(Some(TaskStatus::Taken), None, "late"),
            false,
            Some(&late_principal),
        )
        .unwrap();
    assert_eq!(
        taken.assigned_principal.as_deref(),
        Some(late_principal.as_str())
    );
    assert_eq!(
        store.load_tasks("proj")[0].assigned_principal.as_deref(),
        Some(late_principal.as_str())
    );
    // From then on the name alone no longer proves who the assignee is.
    assert!(matches!(
        hub.update_task(
            "proj",
            task.id,
            patch(Some(TaskStatus::Done), None, "late"),
            false,
            Some("pr_someone_else"),
        ),
        Err(TaskError::Forbidden)
    ));
}
