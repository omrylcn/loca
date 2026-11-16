//! The Lead bound to its principal: a database written before the Lead's
//! principal was stored opens with none, and the old title grants no task
//! authority until the Lead is named again.

use std::sync::Arc;

use super::super::Hub;
use super::critical_restart_hub;
use crate::store::Store;

const V0_13_0_DUMP: &str = include_str!("../../../tests/fixtures/upgrade-v0.13.0/v0.13.0.sql");
/// The fixture's `fx-bot`, the Lead of `fx-team`.
const FX_BOT: &str = "pr_0db74bb3c40c294f1f1d704e468ef1fc";

#[test]
fn an_older_database_opens_with_no_lead_principal_and_the_old_title_assigns_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.db");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(V0_13_0_DUMP)
        .expect("v0.13.0 dump");

    let store = Arc::new(Store::open(path.to_str()).expect("upgrade"));
    // Tasks keep every field and gain no authority record.
    for task in store.load_tasks("fx-team") {
        assert_eq!(task.assigned_by, None, "#{}", task.id);
        assert_eq!(task.assigned_as, None, "#{}", task.id);
        assert_eq!(task.created_as, None, "#{}", task.id);
        assert_eq!(task.created_principal, None, "#{}", task.id);
    }
    let hub: Hub = critical_restart_hub(store, "lead-root");
    let settings = hub.settings("fx-team");
    assert_eq!(settings.lead.as_deref(), Some("fx-bot"));
    assert_eq!(settings.lead_principal, None);
    assert!(
        !hub.is_task_lead("fx-team", "fx-bot", Some(FX_BOT)),
        "a Lead named by name alone holds task authority after an upgrade"
    );

    // Naming the Lead again binds the title to the one principal wearing it.
    let settings = hub
        .set_lead("fx-team", Some("fx-bot".into()), "operator")
        .unwrap();
    assert_eq!(settings.lead_principal.as_deref(), Some(FX_BOT));
    assert!(hub.is_task_lead("fx-team", "fx-bot", Some(FX_BOT)));
    assert!(!hub.is_task_lead("fx-team", "fx-bot", None));
    assert!(!hub.is_task_lead("fx-team", "fx-bot", Some("pr_someone_else")));
}

/// Where nothing has a principal (no database), the name is what there is,
/// as for operators there (`Hub::is_loca_operator`).
#[test]
fn without_a_database_the_lead_is_recognised_by_its_name() {
    let hub = critical_restart_hub(Arc::new(Store::open(None).unwrap()), "lead-root");
    hub.set_lead("proj", Some("bob".into()), "operator")
        .unwrap();
    assert_eq!(hub.settings("proj").lead_principal, None);
    assert!(hub.is_task_lead("proj", "bob", None));
    assert!(!hub.is_task_lead("proj", "eve", None));
}
