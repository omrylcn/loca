use super::*;

#[test]
fn selected_context_restart_and_receipt_fences() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("wiki.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store.configure_wiki("r", "editor", false, 30).unwrap();
    let edit = WikiPageEdit {
        expected_revision: 0,
        title: "Overview".into(),
        body: "one".into(),
    };
    store
        .edit_wiki_page("r", "operator", "overview", &edit, 1)
        .unwrap();
    let request = WikiContextSelection {
        slugs: vec!["overview".into()],
        reason: "need the decision".into(),
    };
    store
        .select_wiki_context("r", "reader", &request, 2)
        .unwrap();
    store
        .select_wiki_context("r", "reader", &request, 3)
        .unwrap();
    let packet = store.wiki_context_for("r", "reader").unwrap();
    assert_eq!(packet["selection_version"], 1);
    assert_eq!(packet["pages"].as_array().unwrap().len(), 1);
    let ack = WikiContextAck {
        context_id: "native-generation-1".into(),
        packet_id: "a".repeat(64),
        revision: 1,
        selection_version: 1,
    };
    store.accept_wiki_context("r", "reader", &ack, 4).unwrap();
    drop(store);
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    assert_eq!(
        store.wiki_context_for("r", "reader").unwrap()["slugs"],
        serde_json::json!(["overview"])
    );
    store.accept_wiki_context("r", "reader", &ack, 5).unwrap();
    let stale = WikiContextAck {
        context_id: "native-generation-2".into(),
        ..ack
    };
    store
        .select_wiki_context(
            "r",
            "reader",
            &WikiContextSelection {
                slugs: vec!["missing".into()],
                reason: "new need".into(),
            },
            6,
        )
        .unwrap();
    assert_eq!(
        store.accept_wiki_context("r", "reader", &stale, 7),
        Err(WikiError::Conflict)
    );
    let packet = store.wiki_context_for("r", "reader").unwrap();
    assert_eq!(packet["missing"], serde_json::json!(["missing"]));
    assert!(packet["pages"].as_array().unwrap().is_empty());
    assert_eq!(
        store.wiki_context_for("r", "other-reader").unwrap()["selection_version"],
        0
    );
}

#[test]
fn native_readiness_expires_and_clock_rollback_never_revalidates_it() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("wiki.db");
    let store = Store::open(Some(path.to_str().unwrap())).unwrap();
    store
        .wiki_native_ready("r", "editor", "codex", 200_000)
        .unwrap();
    assert!(store
        .wiki_editor_runtime_ready("r", "editor", 200_000)
        .unwrap());
    assert!(!store
        .wiki_editor_runtime_ready("r", "editor", 199_999)
        .unwrap());
    assert!(!store
        .wiki_editor_runtime_ready("r", "editor", 320_001)
        .unwrap());
    assert!(!store
        .wiki_editor_runtime_ready("other", "editor", 200_000)
        .unwrap());
    assert_eq!(
        store.wiki_native_ready("r", "editor", "generic", 200_000),
        Err(WikiError::InvalidInput)
    );
}
