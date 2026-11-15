//! The open rewrites the scope keys earlier versions built from a membership
//! key (`mb:<key>`) or an invite (`dv:<invite>`) in `messages.principal`,
//! `message_reactions.principal` and `attachment_pending.uploader` to the id
//! form (`mb:<mbid_>`, `dv:<ivid_>`): in one transaction, with
//! `secure_delete` and the owed WAL clean-up, idempotently, and not at all
//! when it fails.
//!
//! The rows are written the way the code before this change wrote them: the
//! same store calls with the scope keys it passed (`mb:<key>` for an invite
//! session's post, reaction and upload; `dv:<invite>` for an older
//! WebSocket post with an unbound invite).

use rusqlite::Connection;

use super::super::Store;
use super::invite_delivery::occurrences_by_owner;
use super::{contains_bytes, files_containing, hub_on, sql_values_containing};

/// A send time shaped like a real one (milliseconds since the epoch).
const AT: u64 = 1_800_000_000_000;

/// A membership key and an invite shaped like the server's around markers
/// nothing else in the file contains, so a byte search is meaningful.
const MEMBER_KEY: &str = "mb_KEYSCOPEMEMBERQ7ZK3W9X2M";
const INVITE: &str = "dv_KEYSCOPEDAVETQ7ZK3W9X2M";

const ROOM: &str = "ops";

fn member_id() -> String {
    super::super::hashed_id("mbid_", &format!("member:{MEMBER_KEY}"))
}

fn invite_id() -> String {
    hub_on(Store::open(None).unwrap()).invite_management_id(INVITE)
}

fn message(id: u64, sender: &str, text: &str) -> protocol::Message {
    protocol::Message {
        id,
        room: ROOM.into(),
        sender: sender.into(),
        sender_type: protocol::SenderType::User,
        target: None,
        text: text.into(),
        kind: protocol::MessageKind::Say,
        reply_to: None,
        reply_to_sender: None,
        ts: AT + id,
        attachments: Vec::new(),
        sender_principal: None,
    }
}

/// What the code before this change stored: ada's post, reaction and
/// upload over an invite session (`mb:<key>`), an older socket post with an
/// unbound invite (`dv:<invite>`), and rows under keys that carry no secret.
fn older_rows(store: &Store) {
    store
        .add_member(MEMBER_KEY, "ada", "user", AT, "master")
        .unwrap();
    let ada = format!("mb:{MEMBER_KEY}");
    let legacy = format!("dv:{INVITE}");
    for (id, sender, scope) in [
        (1, "ada", Some((ada.as_str(), "op-ada"))),
        (2, "legacy", Some((legacy.as_str(), "op-legacy"))),
        (3, "bea", Some(("pr:pr_bea", "op-pr"))),
        (4, "bea", Some(("label:user:bea", "op-label"))),
        (5, "bea", Some(("user:bea", "op-rest"))),
        (6, "Master", Some(("@master", "op-master"))),
        (7, "bea", None),
    ] {
        store
            .insert_message(&message(id, sender, &format!("words {id}")), scope)
            .unwrap();
    }
    store
        .set_message_reaction(ROOM, 3, &ada, "ada", "✓", true, AT + 10)
        .unwrap();
    store
        .set_message_reaction(ROOM, 1, "reactor:bea", "bea", "♥", true, AT + 11)
        .unwrap();
    store
        .put_pending_attachment(
            ROOM,
            &ada,
            "note.txt",
            "text/plain",
            b"a note ada uploaded before the upgrade\n",
            AT + 12,
            u64::MAX,
            u64::MAX,
        )
        .unwrap();
}

/// Every value of the three columns, by row.
fn scopes(conn: &Connection) -> Vec<String> {
    let mut out = Vec::new();
    for sql in [
        "SELECT 'messages ' || id || ' ' || ifnull(principal, '-') || ' ' || ifnull(op_id, '-')
         FROM messages ORDER BY id",
        "SELECT 'reaction ' || message_id || ' ' || emoji || ' ' || principal
         FROM message_reactions ORDER BY message_id, emoji, principal",
        "SELECT 'pending ' || room || ' ' || uploader FROM attachment_pending ORDER BY sha",
    ] {
        let mut statement = conn.prepare(sql).unwrap();
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap();
        out.extend(rows.map(Result::unwrap));
    }
    out
}

fn expected_after() -> Vec<String> {
    let member = member_id();
    let invite = invite_id();
    vec![
        format!("messages 1 mb:{member} op-ada"),
        format!("messages 2 dv:{invite} op-legacy"),
        "messages 3 pr:pr_bea op-pr".into(),
        "messages 4 label:user:bea op-label".into(),
        "messages 5 user:bea op-rest".into(),
        "messages 6 @master op-master".into(),
        "messages 7 - -".into(),
        "reaction 1 ♥ reactor:bea".into(),
        format!("reaction 3 ✓ mb:{member}"),
        format!("pending {ROOM} mb:{member}"),
    ]
}

/// After the upgrade the same member names itself `mb:<mbid_>`
/// (`Hub::operation_scope`, its REST `legacy` key, the reaction and upload
/// routes): the post's retry finds its message, the reaction is still its
/// own to take back, the pending upload is still its own.
#[test]
fn the_open_names_older_senders_reactors_and_uploaders_by_id() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("scopes.db");
    let store = Store::open(path.to_str()).unwrap();
    older_rows(&store);
    drop(store);

    let store = Store::open(path.to_str()).unwrap();
    let conn = store.conn().unwrap();
    assert_eq!(scopes(&conn), expected_after());
    drop(conn);
    let member = format!("mb:{}", member_id());
    let retried = store
        .message_by_operation(ROOM, &member, "op-ada")
        .unwrap()
        .expect("the retry finds the original");
    assert_eq!((retried.id, retried.text.as_str()), (1, "words 1"));
    assert!(store
        .message_by_operation(ROOM, &format!("mb:{MEMBER_KEY}"), "op-ada")
        .unwrap()
        .is_none());
    let reactions = store
        .set_message_reaction(ROOM, 3, &member, "ada", "✓", false, AT + 20)
        .unwrap();
    assert!(
        reactions.is_empty(),
        "ada takes back the reaction she made before the upgrade: {reactions:?}"
    );
    assert!(!store.join_key_checkpoint_owed().unwrap());
}

/// What a byte search can show after the rewrite. The membership itself
/// keeps only its key's hash, so once the old scope keys are rewritten no
/// value of any column holds the key or the invite, no page of the file
/// (live or free) holds either, and neither the file nor the WAL holds the
/// old scope key `mb:<key>`. The older rows are still in the WAL when the
/// upgraded open runs (another connection keeps the file open, so closing
/// the store did not checkpoint them).
#[test]
fn after_the_rewrite_no_key_is_left_in_the_three_tables_or_the_wal() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("residue.db");
    let wal = directory.path().join("residue.db-wal");
    let store = Store::open(path.to_str()).unwrap();
    // Neighbours, so the rows share pages with others.
    for id in 100..140 {
        store
            .insert_message(
                &message(id, "bea", "a neighbour"),
                Some(("pr:pr_bea", &format!("op-{id}"))),
            )
            .unwrap();
    }
    older_rows(&store);
    // Attached to the file (a query, no transaction left open), so closing
    // the store is not the last close and does not checkpoint.
    let holder = Connection::open(&path).unwrap();
    holder
        .query_row("SELECT count(*) FROM messages", [], |r| r.get::<_, i64>(0))
        .unwrap();
    drop(store);
    let old_scope = format!("mb:{MEMBER_KEY}");
    assert!(
        contains_bytes(&std::fs::read(&wal).unwrap(), old_scope.as_bytes()),
        "the older rows are in the WAL before the upgrade"
    );

    let store = Store::open(path.to_str()).unwrap();
    assert!(!store.join_key_checkpoint_owed().unwrap());
    let conn = store.conn().unwrap();
    assert_eq!(
        sql_values_containing(&conn, MEMBER_KEY),
        Vec::<String>::new()
    );
    assert_eq!(sql_values_containing(&conn, INVITE), Vec::<String>::new());
    assert_eq!(
        occurrences_by_owner(&conn, &path, MEMBER_KEY),
        Default::default(),
        "no page holds the key"
    );
    drop(conn);
    for needle in [MEMBER_KEY, old_scope.as_str(), INVITE, "dv:dv_"] {
        assert_eq!(
            files_containing(&path, needle),
            Vec::<String>::new(),
            "{needle}"
        );
    }
    drop(holder);
}

/// A second open finds nothing to rewrite and changes nothing, records no
/// chore, and the values stay as the first open left them.
#[test]
fn a_second_open_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("twice.db");
    let store = Store::open(path.to_str()).unwrap();
    older_rows(&store);
    drop(store);
    let store = Store::open(path.to_str()).unwrap();
    let first = scopes(&store.conn().unwrap());
    assert_eq!(first, expected_after());
    drop(store);

    let store = Store::open(path.to_str()).unwrap();
    let conn = store.conn().unwrap();
    assert_eq!(scopes(&conn), first);
    let chores: i64 = conn
        .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chores, 0);
}

/// A failure anywhere in the rewrite (here: the last of the three tables)
/// leaves every old value, records no chore and fails the open; once the
/// cause is gone the next open rewrites everything.
#[test]
fn an_injected_failure_leaves_the_old_values_and_fails_the_open() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("fails.db");
    let store = Store::open(path.to_str()).unwrap();
    older_rows(&store);
    let before = scopes(&store.conn().unwrap());
    drop(store);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TRIGGER refuse_uploader BEFORE UPDATE OF uploader ON attachment_pending
         BEGIN SELECT RAISE(ABORT, 'injected rewrite failure'); END;",
    )
    .unwrap();
    drop(conn);

    let error = Store::open(path.to_str())
        .err()
        .expect("the open fails with the rewrite");
    assert!(
        error.to_string().contains("injected rewrite failure"),
        "{error}"
    );
    let conn = Connection::open(&path).unwrap();
    assert_eq!(scopes(&conn), before);
    assert!(before.contains(&format!("messages 1 mb:{MEMBER_KEY} op-ada")));
    let chores: i64 = conn
        .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
        .unwrap();
    assert_eq!(chores, 0, "nothing was recorded either");
    conn.execute_batch("DROP TRIGGER refuse_uploader").unwrap();
    drop(conn);

    let store = Store::open(path.to_str()).unwrap();
    assert_eq!(scopes(&store.conn().unwrap()), expected_after());
}

/// A database an older build wrote to after a rollback can hold the same
/// thing under both forms. The rewrite keeps one of each: the member's
/// reaction once, and the operation answered by the message the id form
/// already names; the other message stays, without the key.
#[test]
fn a_thing_stored_under_both_forms_is_kept_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("both.db");
    let store = Store::open(path.to_str()).unwrap();
    let raw = format!("mb:{MEMBER_KEY}");
    let by_id = format!("mb:{}", member_id());
    store
        .insert_message(&message(1, "ada", "first"), Some((&by_id, "op-twice")))
        .unwrap();
    store
        .insert_message(&message(2, "ada", "again"), Some((&raw, "op-twice")))
        .unwrap();
    store
        .insert_message(&message(3, "bea", "hello"), None)
        .unwrap();
    for (scope, at) in [(&by_id, AT + 1), (&raw, AT + 2)] {
        store
            .set_message_reaction(ROOM, 3, scope, "ada", "✓", true, at)
            .unwrap();
    }
    drop(store);

    let store = Store::open(path.to_str()).unwrap();
    assert_eq!(
        scopes(&store.conn().unwrap()),
        [
            format!("messages 1 {by_id} op-twice"),
            "messages 2 - -".to_string(),
            "messages 3 - -".to_string(),
            format!("reaction 3 ✓ {by_id}"),
        ]
    );
    let answered = store
        .message_by_operation(ROOM, &by_id, "op-twice")
        .unwrap()
        .expect("answered");
    assert_eq!(answered.id, 1);
}

/// A reader holding the file keeps the WAL from being emptied: the rewrite
/// is committed and the open goes on, and the clean-up stays owed, as for a
/// cleared delivery copy.
#[test]
fn a_busy_database_does_not_stop_the_open_and_keeps_the_clean_up_owed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("busy.db");
    let store = Store::open(path.to_str()).unwrap();
    older_rows(&store);
    drop(store);
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM messages;")
        .unwrap();

    let store = Store::open(path.to_str()).expect("a busy WAL does not stop the start");
    assert_eq!(scopes(&store.conn().unwrap()), expected_after());
    assert!(store.join_key_checkpoint_owed().unwrap());
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    drop(store);
    let store = Store::open(path.to_str()).unwrap();
    assert!(!store.join_key_checkpoint_owed().unwrap());
}
