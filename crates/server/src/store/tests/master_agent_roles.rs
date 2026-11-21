//! Widening `principals` so it can hold the `master_agent` role: a database
//! written before the rank existed is rebuilt once, row for row, and every
//! table that names a principal still names the same one.

use rusqlite::Connection;

use super::super::Store;

/// The role list every version before the Master Agent rank wrote, and
/// this version's.
const OLD_ROLES: &str = "IN ('master', 'smaster', 'member')";
const NEW_ROLES: &str = "IN ('master', 'smaster', 'member', 'master_agent')";

fn principals_sql(conn: &Connection) -> String {
    conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'principals'",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// Every column of every principal, rendered with its type, by id.
fn principal_rows(conn: &Connection) -> Vec<String> {
    let mut st = conn
        .prepare("SELECT * FROM principals ORDER BY id")
        .unwrap();
    let width = st.column_count();
    st.query_map([], |r| {
        let mut row = Vec::new();
        for i in 0..width {
            row.push(format!("{:?}", r.get::<_, rusqlite::types::Value>(i)?));
        }
        Ok(row.join(" | "))
    })
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

/// A database whose `principals` has the old CHECK, holding a member, a
/// Smaster (one revoked) and their credentials.
fn older_database(path: &std::path::Path) {
    {
        let store = Store::open(path.to_str()).unwrap();
        store
            .add_member("old-member-key", "worker", "agent", 1, "master")
            .unwrap();
        store.add_smaster("sm_old-smaster", "deputy", 2).unwrap();
        store.add_smaster("sm_gone-smaster", "former", 3).unwrap();
        store
            .revoke_smaster(&super::super::secret_hash("sm_gone-smaster"), 4)
            .unwrap();
    }
    // This version's own table (with every column added since), its role
    // list put back to what every earlier version wrote. A later column
    // holds a value, so the copy is seen to keep every column.
    let conn = Connection::open(path).unwrap();
    conn.execute(
        "UPDATE principals SET full_name = 'Ada Worker', icon = 'bolt' WHERE display_name = 'worker'",
        [],
    )
    .unwrap();
    let old = principals_sql(&conn).replace(NEW_ROLES, OLD_ROLES);
    assert!(old.contains(OLD_ROLES), "{old}");
    conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
    conn.execute_batch(&format!(
        "BEGIN;
         CREATE TABLE principals_old AS SELECT * FROM principals;
         DROP TABLE principals;
         {old};
         CREATE UNIQUE INDEX principals_one_master
             ON principals(building_role) WHERE building_role = 'master' AND disabled_at IS NULL;
         INSERT INTO principals SELECT * FROM principals_old;
         DROP TABLE principals_old;
         COMMIT;"
    ))
    .unwrap();
    assert!(!principals_sql(&conn).contains("master_agent"));
}

#[test]
fn an_older_principals_table_is_widened_once_row_for_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("older.db");
    older_database(&path);
    let before = principal_rows(&Connection::open(&path).unwrap());
    assert_eq!(before.len(), 3, "{before:?}");

    let store = Store::open(path.to_str()).unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    assert!(principals_sql(&conn).contains("'master_agent'"));
    assert_eq!(principal_rows(&conn), before, "every row kept as it was");
    let dangling: i64 = conn
        .prepare("PRAGMA foreign_key_check")
        .unwrap()
        .query_map([], |_| Ok(()))
        .unwrap()
        .count() as i64;
    assert_eq!(dangling, 0, "every reference still names its principal");
    let one_master: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name = 'principals_one_master'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(one_master, 1, "the one-Master index is back");

    // The member and the Smaster still sign in as themselves.
    let store = Store::open(path.to_str()).unwrap();
    assert_eq!(
        store
            .principal_for_credential("old-member-key")
            .map(|p| p.display_name),
        Some("worker".to_string())
    );
    assert_eq!(
        store
            .principal_for_credential("sm_old-smaster")
            .map(|p| p.display_name),
        Some("deputy".to_string())
    );
    assert!(store.principal_for_credential("sm_gone-smaster").is_none());

    // And the new role can be written.
    store
        .add_master_agent("ma_new-agent", "automaton", "pr_master", 5)
        .unwrap();
    assert_eq!(
        store
            .principal_for_credential("ma_new-agent")
            .map(|p| p.role),
        Some(crate::store::BuildingRole::MasterAgent)
    );
    drop(store);

    // A second open changes nothing.
    let sql = principals_sql(&Connection::open(&path).unwrap());
    let rows = principal_rows(&Connection::open(&path).unwrap());
    drop(Store::open(path.to_str()).unwrap());
    assert_eq!(principals_sql(&Connection::open(&path).unwrap()), sql);
    assert_eq!(principal_rows(&Connection::open(&path).unwrap()), rows);
}

#[test]
fn a_reference_that_was_already_dangling_does_not_stop_the_widening() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dangling.db");
    older_database(&path);
    {
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = OFF;
             INSERT INTO credentials (id, principal_id, label, secret_hash, created_at)
             VALUES ('cr_orphan', 'pr_nobody', 'orphan', 'orphan-hash', 9);",
        )
        .unwrap();
    }
    let before = principal_rows(&Connection::open(&path).unwrap());
    drop(Store::open(path.to_str()).unwrap());
    let conn = Connection::open(&path).unwrap();
    assert!(principals_sql(&conn).contains("'master_agent'"));
    assert_eq!(principal_rows(&conn), before);
}

#[test]
fn a_widening_that_fails_part_way_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("half.db");
    older_database(&path);
    // The rebuild's own table name is already taken: its first step fails.
    Connection::open(&path)
        .unwrap()
        .execute_batch("CREATE TABLE principals_widened (x INTEGER);")
        .unwrap();
    let before = principal_rows(&Connection::open(&path).unwrap());
    assert!(Store::open(path.to_str()).is_err(), "the open refuses");
    let conn = Connection::open(&path).unwrap();
    assert!(
        !principals_sql(&conn).contains("master_agent"),
        "the old table is untouched"
    );
    assert_eq!(principal_rows(&conn), before);
}
