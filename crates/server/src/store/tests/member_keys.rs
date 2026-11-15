//! Membership keys and invites stop being stored. These build `members`,
//! `invites` and `member_suspensions` the way the builds up to this one left
//! them on disk (each keyed by the key or the invite itself, a membership's
//! identity bound in `principals`/`credentials`, a WAL not yet checkpointed),
//! with the rows that name a member by its key elsewhere, and open them here.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use super::super::{hashed_id, keys::fingerprint, secret_hash, Store};
use super::{
    files_containing, finish_legacy_files, hub_on, identity_rows, legacy_build_connection,
    schema_dump, sql_values_containing, LegacyLayout,
};

/// When the upgrade opens the files (milliseconds since the epoch).
const NOW: u64 = 1_800_000_000_000;
const HOUR: u64 = 60 * 60 * 1_000;

/// A key or invite shaped like the server's (`mb_`/`dv_` and 64 characters)
/// around a marker nothing else in the file contains, so a byte search is
/// meaningful.
fn synthetic(prefix: &str, tag: &str) -> String {
    let mut secret = format!("{prefix}QZXJ-synthetic-member-secret-{tag}-");
    while secret.len() < 67 {
        secret.push('7');
    }
    secret
}

/// One membership as the builds up to this one recorded it.
struct OldMember {
    key: String,
    issued_as: &'static str,
    renamed_to: Option<&'static str>,
    kind: &'static str,
    joined_at: u64,
    revoked_at: Option<u64>,
    suspended_at: Option<u64>,
    /// Its principal and credential were bound by an earlier open (every
    /// build since identity v2); `false` is a row from before that.
    bound: bool,
    last_used_at: Option<u64>,
    /// A key added under Profile -> Keys.
    device: Option<&'static str>,
}

impl OldMember {
    fn name(&self) -> &'static str {
        self.renamed_to.unwrap_or(self.issued_as)
    }
    fn derived(&self, prefix: &str) -> String {
        hashed_id(prefix, &format!("member:{}", self.key))
    }
    fn live(&self) -> bool {
        self.revoked_at.is_none() && self.suspended_at.is_none()
    }
    /// The part of the key the ledger never shows.
    fn secret_tail(&self) -> &str {
        &self.key[7..]
    }
}

/// One invite as the builds up to this one recorded it.
struct OldInvite {
    invite: String,
    room: &'static str,
    /// The member it seats, by its place in `old_members()`; `None` for an
    /// invite from before invites named their member.
    member: Option<usize>,
    name: &'static str,
    issued_at: u64,
    issued_by: &'static str,
    revoked_at: Option<u64>,
    /// Its delivery copy is still held.
    copy: bool,
    first_used_at: Option<u64>,
}

impl OldInvite {
    fn id(&self) -> String {
        hashed_id("ivid_", &format!("invite:{}", self.invite))
    }
    fn secret_tail(&self) -> &str {
        &self.invite[7..]
    }
}

/// A live member with a second key, a revoked one, a suspended one, a
/// renamed one, one admitted by a join request whose key copy is still
/// collectable, and one from before identity v2 (never bound).
fn old_members() -> Vec<OldMember> {
    let member = |tag: &str, issued_as, kind, joined_at| OldMember {
        key: synthetic("mb_", tag),
        issued_as,
        renamed_to: None,
        kind,
        joined_at,
        revoked_at: None,
        suspended_at: None,
        bound: true,
        last_used_at: None,
        device: None,
    };
    vec![
        OldMember {
            last_used_at: Some(NOW - 2 * HOUR),
            device: Some("ak_synthetic-device-credential-for-ada"),
            ..member("ada", "ada", "user", NOW - 50 * HOUR)
        },
        OldMember {
            revoked_at: Some(NOW - 10 * HOUR),
            last_used_at: Some(NOW - 11 * HOUR),
            ..member("bea", "bea", "agent", NOW - 49 * HOUR)
        },
        OldMember {
            suspended_at: Some(NOW - 5 * HOUR),
            ..member("cem", "cem", "agent", NOW - 48 * HOUR)
        },
        OldMember {
            renamed_to: Some("dora"),
            ..member("dora", "dora-first", "user", NOW - 47 * HOUR)
        },
        member("ece", "ece", "agent", NOW - HOUR),
        OldMember {
            bound: false,
            ..member("hale", "hale", "agent", NOW - 46 * HOUR)
        },
    ]
}

/// ada's invite whose delivery is still owed and one already used, bea's
/// ended with her membership, cem's ended by the suspension, dora's past
/// its delivery window, and one from before invites named their member.
fn old_invites() -> Vec<OldInvite> {
    let invite = |tag: &str, room, member, name, issued_at| OldInvite {
        invite: synthetic("dv_", tag),
        room,
        member,
        name,
        issued_at,
        issued_by: "master",
        revoked_at: None,
        copy: false,
        first_used_at: None,
    };
    vec![
        OldInvite {
            copy: true,
            ..invite("ada-owed", "parlor", Some(0), "ada", NOW - HOUR)
        },
        OldInvite {
            first_used_at: Some(NOW - 2 * HOUR),
            issued_by: "smaster:selin",
            ..invite("ada-used", "ops", Some(0), "ada", NOW - 3 * HOUR)
        },
        OldInvite {
            revoked_at: Some(NOW - 10 * HOUR),
            ..invite("bea", "parlor", Some(1), "bea", NOW - 20 * HOUR)
        },
        OldInvite {
            revoked_at: Some(NOW - 5 * HOUR),
            ..invite("cem", "parlor", Some(2), "cem", NOW - 6 * HOUR)
        },
        invite("dora", "ops", Some(3), "dora-first", NOW - 30 * HOUR),
        invite("unbound", "parlor", None, "ivo", NOW - 40 * HOUR),
    ]
}

/// An `invites` row after the upgrade: hash, fingerprint, room, member id,
/// name, issue, issuer, revocation, delivery copy and first use.
type StoredInvite = (
    String,
    String,
    String,
    String,
    String,
    u64,
    String,
    Option<u64>,
    Option<String>,
    Option<u64>,
);

/// The request whose approval admitted ece: approved an hour before the
/// upgrade, collected, never acknowledged, so its key copy is still held.
const ECE_REQUEST: &str = "jr_member_key_fixture_ece";

/// Write the member tables as the builds up to this one left them, with the
/// rows elsewhere that name a member by its key, and leave the files at
/// `legacy.db` in `layout`. Returns the path and the identity rows as they
/// were.
fn legacy_member_files(dir: &Path, layout: LegacyLayout) -> (PathBuf, Vec<Vec<String>>) {
    let members = old_members();
    let invites = old_invites();
    let build = dir.join("build.db");
    drop(Store::open_at(build.to_str(), NOW - 60 * HOUR).expect("current schema"));
    let conn = legacy_build_connection(&build);
    conn.execute_batch(super::super::LEGACY_MEMBER_TABLES)
        .expect("the older member tables");
    conn.execute_batch(
        "ALTER TABLE invites ADD COLUMN delivery_token TEXT;
         ALTER TABLE invites ADD COLUMN first_used_at INTEGER;",
    )
    .expect("the delivery columns of the build before this one");
    for m in &members {
        let hash = secret_hash(&m.key);
        conn.execute(
            "INSERT INTO members (token, name, kind, joined_at, admitted_by, revoked_at)
             VALUES (?1, ?2, ?3, ?4, 'master', NULL)",
            params![m.key, m.issued_as, m.kind, m.joined_at],
        )
        .unwrap();
        if m.bound {
            let principal_kind = if m.kind == "agent" { "agent" } else { "human" };
            conn.execute(
                "INSERT INTO principals (id, display_name, kind, building_role, created_at)
                 VALUES (?1, ?2, ?3, 'member', ?4)",
                params![m.derived("pr_"), m.issued_as, principal_kind, m.joined_at],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO credentials
                 (id, principal_id, label, secret_hash, created_at, last_used_at, revoked_at,
                  legacy_source)
                 VALUES (?1, ?2, 'Member access', ?3, ?4, ?5, NULL, ?6)",
                params![
                    m.derived("cr_"),
                    m.derived("pr_"),
                    hash,
                    m.joined_at,
                    m.last_used_at,
                    format!("member:{hash}")
                ],
            )
            .unwrap();
        }
        if let Some(device) = m.device {
            conn.execute(
                "INSERT INTO credentials
                 (id, principal_id, label, secret_hash, created_at, last_used_at)
                 VALUES (?1, ?2, 'Laptop', ?3, ?4, ?5)",
                params![
                    hashed_id("cr_", &format!("credential:{device}")),
                    m.derived("pr_"),
                    secret_hash(device),
                    m.joined_at + 10,
                    NOW - HOUR
                ],
            )
            .unwrap();
        }
        if let Some(name) = m.renamed_to {
            conn.execute(
                "UPDATE members SET name = ?2 WHERE token = ?1",
                params![m.key, name],
            )
            .unwrap();
            conn.execute(
                "UPDATE principals SET display_name = ?2 WHERE id = ?1",
                params![m.derived("pr_"), name],
            )
            .unwrap();
        }
        if let Some(at) = m.revoked_at {
            conn.execute(
                "UPDATE members SET revoked_at = ?2 WHERE token = ?1",
                params![m.key, at],
            )
            .unwrap();
            conn.execute(
                "UPDATE credentials SET revoked_at = ?2 WHERE principal_id = ?1",
                params![m.derived("pr_"), at],
            )
            .unwrap();
            conn.execute(
                "UPDATE principals SET disabled_at = ?2 WHERE id = ?1",
                params![m.derived("pr_"), at],
            )
            .unwrap();
        }
        if let Some(at) = m.suspended_at {
            conn.execute(
                "INSERT INTO member_suspensions (member_token, suspended_at, reason)
                 VALUES (?1, ?2, 'fixture')",
                params![m.key, at],
            )
            .unwrap();
            conn.execute(
                "UPDATE principals SET disabled_at = ?2 WHERE id = ?1",
                params![m.derived("pr_"), at],
            )
            .unwrap();
        }
    }
    for i in &invites {
        let member = i.member.map(|m| members[m].key.clone()).unwrap_or_default();
        conn.execute(
            "INSERT INTO invites
             (token, room, name, kind, issued_at, issued_by, revoked_at, member, delivery_token,
              first_used_at)
             VALUES (?1, ?2, ?3, 'agent', ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                i.invite,
                i.room,
                i.name,
                i.issued_at,
                i.issued_by,
                i.revoked_at,
                member,
                i.copy.then_some(&i.invite),
                i.first_used_at
            ],
        )
        .unwrap();
    }
    // The rows that named a sender, reactor or uploader by its key: ada's
    // post, reaction and upload over an invite session, and an older socket
    // post with the unbound invite.
    let ada_scope = format!("mb:{}", members[0].key);
    let ivo_scope = format!("dv:{}", invites[5].invite);
    for (id, sender, scope, op) in [
        (1, "ada", &ada_scope, "op-ada"),
        (2, "ivo", &ivo_scope, "op-ivo"),
    ] {
        conn.execute(
            "INSERT INTO messages (id, room, sender, sender_type, text, ts, kind, principal, op_id)
             VALUES (?1, 'parlor', ?2, 'user', 'words', ?3, 'say', ?4, ?5)",
            params![id, sender, NOW - 3 * HOUR + id, scope, op],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO message_reactions (room, message_id, principal, reactor, emoji, created_at)
         VALUES ('parlor', 2, ?1, 'ada', '✓', ?2)",
        params![ada_scope, NOW - 2 * HOUR],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO attachment_pending (sha, room, uploader, name, mime, size, created_at)
         VALUES ('f00d', 'parlor', ?1, 'notes.txt', 'text/plain', 4, ?2)",
        params![ada_scope, NOW - HOUR],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO join_requests
         (id, secret_hash, name, kind, status, created_at, decided_at, decided_by, mb_token,
          bootstrap_delivered_at)
         VALUES (?1, ?2, 'ece', 'agent', 'approved', ?3, ?4, 'master', ?5, ?4)",
        params![
            ECE_REQUEST,
            secret_hash("jrs_member_key_fixture"),
            NOW - 2 * HOUR,
            NOW - HOUR,
            members[4].key
        ],
    )
    .unwrap();
    let identities = identity_rows(&conn);
    if let LegacyLayout::CheckpointedWithFreedPages = layout {
        // Memberships and invites that came and went before the upgrade:
        // their rows were deleted, their cells and pages freed.
        for (n, m) in members.iter().enumerate().cycle().take(300) {
            conn.execute(
                "INSERT OR REPLACE INTO members (token, name, kind, joined_at, admitted_by)
                 VALUES (?1, 'churn', 'agent', 1, 'master')",
                params![format!("{}-copy-{n}", m.key)],
            )
            .unwrap();
        }
        for (n, i) in invites.iter().enumerate().cycle().take(300) {
            conn.execute(
                "INSERT OR REPLACE INTO invites (token, room, name, kind, issued_at, issued_by)
                 VALUES (?1, 'churn', 'churn', 'agent', 1, 'master')",
                params![format!("{}-copy-{n}", i.invite)],
            )
            .unwrap();
        }
        conn.execute_batch(
            "DELETE FROM members WHERE name = 'churn';
             DELETE FROM invites WHERE name = 'churn';",
        )
        .unwrap();
    }
    (finish_legacy_files(dir, &build, conn, layout), identities)
}

/// The secrets an open may still hold after the upgrade: the bounded
/// delivery copies inside their 24 hours, each in its own column. Every
/// other key and invite must be nowhere.
fn allowed_copy(secret: &str) -> Option<&'static str> {
    let members = old_members();
    let invites = old_invites();
    if secret == members[4].key {
        return Some("join_requests.mb_token");
    }
    if secret == invites[0].invite {
        return Some("invites.delivery_token");
    }
    None
}

/// Every key and invite of the fixture, whole and without its fingerprint.
fn fixture_secrets() -> Vec<(String, String)> {
    let mut secrets = Vec::new();
    for m in old_members() {
        secrets.push((m.key.clone(), m.secret_tail().to_string()));
    }
    for i in old_invites() {
        secrets.push((i.invite.clone(), i.secret_tail().to_string()));
    }
    secrets
}

#[test]
fn member_key_migration_keeps_every_identity_and_sign_in() {
    let directory = tempfile::tempdir().unwrap();
    let (path, before) = legacy_member_files(directory.path(), LegacyLayout::PendingWal);
    let members = old_members();
    let invites = old_invites();
    let store = Store::open_at(path.to_str(), NOW).expect("the upgrade opens the database");
    {
        let c = store.conn().unwrap();
        for (table, column) in [
            ("members", "token"),
            ("invites", "token"),
            ("member_suspensions", "member_token"),
        ] {
            assert!(
                !super::super::table_has_column(&c, table, column).unwrap(),
                "{table}.{column} is still a column"
            );
        }
        // Not one principal or credential was changed or lost. The only
        // ones added bind the member an earlier open never bound, with the
        // identifiers v0.13.0 derived from its key.
        let after = identity_rows(&c);
        for row in &before {
            assert!(after.contains(row), "lost or changed: {row:?}");
        }
        let hale = &members[5];
        let added = after.iter().filter(|row| !before.contains(row)).count();
        assert_eq!(added, 2, "one principal and one credential, for hale");
        let bound: (String, String, String) = c
            .query_row(
                "SELECT p.display_name, p.building_role, c.label
                 FROM principals p JOIN credentials c ON c.principal_id = p.id
                 WHERE p.id = ?1 AND c.id = ?2 AND c.legacy_source = ?3",
                params![
                    hale.derived("pr_"),
                    hale.derived("cr_"),
                    format!("member:{}", secret_hash(&hale.key))
                ],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("hale is bound with v0.13.0's identifiers");
        assert_eq!(
            bound,
            (
                "hale".to_string(),
                "member".to_string(),
                "Legacy member access".to_string()
            )
        );

        for m in &members {
            let row: (
                String,
                String,
                String,
                String,
                String,
                String,
                u64,
                Option<u64>,
            ) = c
                .query_row(
                    "SELECT secret_hash, principal_id, credential_id, fingerprint, name, kind,
                            joined_at, revoked_at
                     FROM members WHERE id = ?1",
                    [m.derived("mbid_")],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                        ))
                    },
                )
                .unwrap_or_else(|error| panic!("{} under its mbid_: {error}", m.name()));
            assert_eq!(
                row,
                (
                    secret_hash(&m.key),
                    m.derived("pr_"),
                    m.derived("cr_"),
                    fingerprint(&m.key),
                    m.name().to_string(),
                    m.kind.to_string(),
                    m.joined_at,
                    m.revoked_at
                ),
                "{}",
                m.name()
            );
        }
        for i in &invites {
            let member_id = i
                .member
                .map(|m| members[m].derived("mbid_"))
                .unwrap_or_default();
            let row: StoredInvite = c
                .query_row(
                    "SELECT secret_hash, fingerprint, room, member_id, name, issued_at, issued_by,
                            revoked_at, delivery_token, first_used_at
                     FROM invites WHERE id = ?1",
                    [i.id()],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                            r.get(7)?,
                            r.get(8)?,
                            r.get(9)?,
                        ))
                    },
                )
                .unwrap_or_else(|error| panic!("{} in {}: {error}", i.name, i.room));
            assert_eq!(
                row,
                (
                    secret_hash(&i.invite),
                    fingerprint(&i.invite),
                    i.room.to_string(),
                    member_id,
                    i.name.to_string(),
                    i.issued_at,
                    i.issued_by.to_string(),
                    i.revoked_at,
                    // The delivery copy and the first use are carried over
                    // exactly: the copy is still the bounded one.
                    i.copy.then(|| i.invite.clone()),
                    i.first_used_at
                ),
                "{} in {}",
                i.name,
                i.room
            );
        }
        let suspended: Vec<(String, u64)> = c
            .prepare("SELECT member_id, suspended_at FROM member_suspensions")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            suspended,
            vec![(
                members[2].derived("mbid_"),
                members[2].suspended_at.unwrap()
            )]
        );
        // The rows that named ada by her key name her by her id.
        let ada = format!("mb:{}", members[0].derived("mbid_"));
        let scopes: Vec<String> = c
            .prepare(
                "SELECT principal FROM messages WHERE principal IS NOT NULL
                 UNION ALL SELECT principal FROM message_reactions
                 UNION ALL SELECT uploader FROM attachment_pending",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(
            scopes,
            vec![
                ada.clone(),
                format!("dv:{}", invites[5].id()),
                ada.clone(),
                ada
            ]
        );
    }
    // The ledger shows the ids and fingerprints it showed before, and a
    // member's last use from any of its keys.
    let ledger = store.key_ledger(NOW, 7 * 24 * HOUR).unwrap();
    for m in &members {
        let entry = ledger
            .iter()
            .find(|k| k.kind == "member" && k.id == m.derived("mbid_"))
            .unwrap_or_else(|| panic!("{} is not in the ledger", m.name()));
        assert_eq!(entry.fingerprint, fingerprint(&m.key));
        assert_eq!(entry.for_name.as_deref(), Some(m.name()));
        assert_eq!(entry.revoked_at, m.revoked_at);
    }
    let ada = ledger
        .iter()
        .find(|k| k.id == members[0].derived("mbid_"))
        .unwrap();
    assert_eq!(
        ada.last_used_at,
        Some(NOW - HOUR),
        "the device key's use counts"
    );
    for i in &invites {
        let entry = ledger.iter().find(|k| k.id == i.id()).unwrap();
        assert_eq!(entry.fingerprint, fingerprint(&i.invite));
    }

    // The same keys and invites sign in as the same people.
    let hub = hub_on(store);
    for m in &members {
        let found = hub.member_for_credential(Some(&m.key));
        assert_eq!(
            found.as_ref().map(|found| found.id.clone()),
            m.live().then(|| m.derived("mbid_")),
            "{}'s key",
            m.name()
        );
        if let Some(found) = found {
            assert_eq!(found.name, m.name());
            assert_eq!(found.principal_id, Some(m.derived("pr_")));
        }
    }
    let device = hub
        .member_for_credential(members[0].device)
        .expect("ada's second key");
    assert_eq!(device.id, members[0].derived("mbid_"));
    for (n, i) in invites.iter().enumerate() {
        let live = hub.invite_by_token(&i.invite);
        assert_eq!(live.is_some(), i.revoked_at.is_none(), "invite {n}");
        if let Some(live) = live {
            assert_eq!(live.id, i.id());
            let seat = hub.seat_identity(None, Some(&i.invite), None, None, "x");
            assert_eq!(seat, format!("mb:{}", live.member_id));
        }
    }
    // The unbound invite was bound at the start to a membership minted for
    // its name, never to one of the fixture's members.
    let ivo = hub
        .member_by_name("ivo")
        .expect("a membership for the old invite");
    assert_eq!(ivo.admitted_by, "migration:legacy-invite");
    assert_eq!(
        hub.invite_by_token(&invites[5].invite).unwrap().member_id,
        ivo.id
    );
    // ada's owed invite is still delivered on her next Lobby connect.
    let owed = hub.pending_deliveries(&members[0].derived("mbid_"));
    assert_eq!(
        owed.into_iter()
            .map(|(_, invite)| invite)
            .collect::<Vec<_>>(),
        vec![invites[0].invite.clone()]
    );
}

/// After the upgrade the files hold none of the keys or invites but the
/// bounded copies a delivery still needs; nor does a snapshot taken after
/// it, and a restored snapshot opens with nothing owed and the same keys
/// working.
#[test]
fn member_keys_leave_no_trace_in_the_files_or_a_snapshot_and_a_restored_snapshot_works() {
    for layout in [
        LegacyLayout::PendingWal,
        LegacyLayout::CheckpointedWithFreedPages,
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (path, _) = legacy_member_files(directory.path(), layout);
        let secrets = fixture_secrets();
        // The fixture is meaningful: before the upgrade every one is on disk.
        for (secret, tail) in &secrets {
            assert!(!files_containing(&path, tail).is_empty(), "{secret}");
        }

        let store = Store::open_at(path.to_str(), NOW).expect("upgrade");
        for (secret, tail) in &secrets {
            let expected: Vec<String> = allowed_copy(secret)
                .map(str::to_string)
                .into_iter()
                .collect();
            for needle in [secret.as_str(), tail.as_str()] {
                assert_eq!(
                    sql_values_containing(&store.conn().unwrap(), needle),
                    expected,
                    "{secret}"
                );
                if expected.is_empty() {
                    assert_eq!(
                        files_containing(&path, needle),
                        Vec::<String>::new(),
                        "no byte of the open database, its WAL or shm holds {secret}"
                    );
                }
            }
        }
        assert_eq!(
            store
                .conn()
                .unwrap()
                .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "the clean-up ran and nothing is owed"
        );
        drop(store);

        let script =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/storage_snapshot.py");
        let python = std::env::var("PYTHON").unwrap_or_else(|_| "python3".into());
        let snapshot = directory.path().join("snapshot");
        let backup = std::process::Command::new(&python)
            .arg(&script)
            .arg("backup")
            .arg(&path)
            .arg(&snapshot)
            .arg("--server-stopped")
            .output()
            .expect("python3 runs the snapshot tool");
        assert!(
            backup.status.success(),
            "{}",
            String::from_utf8_lossy(&backup.stderr)
        );
        for (secret, tail) in &secrets {
            if allowed_copy(secret).is_none() {
                assert_eq!(
                    files_containing(&snapshot.join("loca.sqlite3"), tail),
                    Vec::<String>::new(),
                    "a snapshot taken after the upgrade holds {secret}"
                );
            }
        }

        let restored = directory.path().join("restored");
        let restore = std::process::Command::new(&python)
            .arg(&script)
            .arg("restore")
            .arg(&snapshot)
            .arg(&restored)
            .arg("--server-stopped")
            .output()
            .expect("python3 runs the snapshot tool");
        assert!(
            restore.status.success(),
            "{}",
            String::from_utf8_lossy(&restore.stderr)
        );
        let reopened = restored.join("loca.sqlite3");
        let schema = schema_dump(reopened.to_str().unwrap());
        let store =
            Store::open_at(reopened.to_str(), NOW + 1_000).expect("the restored copy opens");
        assert_eq!(
            store
                .conn()
                .unwrap()
                .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "a restored snapshot owes no chore"
        );
        let hub = hub_on(store);
        for m in old_members() {
            assert_eq!(
                hub.member_for_credential(Some(&m.key)).is_some(),
                m.live(),
                "{} after the restore",
                m.name()
            );
        }
        drop(hub);
        assert_eq!(
            schema_dump(reopened.to_str().unwrap()),
            schema,
            "nothing migrated again"
        );
    }
}

#[test]
fn reopening_migrated_member_tables_changes_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_member_files(directory.path(), LegacyLayout::PendingWal);
    let dump = |path: &Path| {
        let c = Connection::open(path).unwrap();
        let mut rows = identity_rows(&c);
        for sql in [
            "SELECT * FROM members ORDER BY id",
            "SELECT * FROM invites ORDER BY id",
            "SELECT * FROM member_suspensions ORDER BY member_id",
            "SELECT principal, op_id FROM messages ORDER BY id",
        ] {
            let mut statement = c.prepare(sql).unwrap();
            let columns = statement.column_count();
            rows.extend(
                statement
                    .query_map([], |row| {
                        (0..columns)
                            .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .map(Result::unwrap),
            );
        }
        rows
    };
    drop(Store::open_at(path.to_str(), NOW).expect("first open"));
    let first = dump(&path);
    let schema = schema_dump(path.to_str().unwrap());
    let store = Store::open_at(path.to_str(), NOW + 1_000).expect("second open");
    assert_eq!(
        store
            .conn()
            .unwrap()
            .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "the second open records no chore"
    );
    drop(store);
    assert_eq!(dump(&path), first, "the second open is a no-op");
    assert_eq!(schema_dump(path.to_str().unwrap()), schema);
}

/// The member tables as the builds up to this one stored them, read
/// straight from the file in their old shape.
fn old_tables(path: &Path) -> Vec<Vec<String>> {
    let c = Connection::open(path).unwrap();
    let mut rows = Vec::new();
    for sql in [
        "SELECT token, name, kind, joined_at, admitted_by, revoked_at FROM members ORDER BY token",
        "SELECT token, room, member, name, issued_at, revoked_at, delivery_token, first_used_at
         FROM invites ORDER BY token",
        "SELECT member_token, suspended_at FROM member_suspensions ORDER BY member_token",
    ] {
        let mut statement = c
            .prepare(sql)
            .expect("the old tables are still readable in their old shape");
        let columns = statement.column_count();
        rows.extend(
            statement
                .query_map([], |row| {
                    (0..columns)
                        .map(|i| Ok(format!("{:?}", row.get_ref(i)?)))
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap()
                .map(Result::unwrap),
        );
    }
    rows
}

#[test]
fn a_failed_member_key_migration_leaves_the_old_tables_and_fails_open() {
    for (case, fault, cause) in [
        (
            "binding the unbound member is refused",
            "CREATE TRIGGER refuse_member_binding BEFORE INSERT ON credentials
             WHEN NEW.legacy_source LIKE 'member:%'
             BEGIN SELECT RAISE(ABORT, 'injected: member binding refused'); END;",
            "injected: member binding refused",
        ),
        (
            "an operator's view over the old column cannot follow the new table",
            "CREATE VIEW member_keys AS SELECT token FROM members;",
            "member_keys",
        ),
        (
            "an invite row cannot be read",
            "UPDATE invites SET issued_at = 'not a time' WHERE name = 'dora-first';",
            "issued_at",
        ),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (path, before) = legacy_member_files(directory.path(), LegacyLayout::PendingWal);
        Connection::open(&path)
            .unwrap()
            .execute_batch(fault)
            .unwrap();
        let old = old_tables(&path);
        let error = match Store::open_at(path.to_str(), NOW) {
            Ok(_) => panic!("{case}: the open must fail when the migration fails"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains(cause), "{case}: the real cause: {error}");
        assert_eq!(
            old_tables(&path),
            old,
            "{case}: the old tables are as they were"
        );
        let c = Connection::open(&path).unwrap();
        assert_eq!(identity_rows(&c), before, "{case}: no binding stayed");
        let leftover: i64 = c
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'members_hashed'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(leftover, 0, "{case}");
        let chores: i64 = c
            .query_row(
                "SELECT count(*) FROM storage_maintenance WHERE task = 'scrub_member_keys'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            chores, 0,
            "{case}: the chore rolled back with the migration"
        );
    }
}

#[test]
fn a_busy_checkpoint_after_the_member_key_migration_is_finished_by_the_next_open() {
    let directory = tempfile::tempdir().unwrap();
    let (path, _) = legacy_member_files(directory.path(), LegacyLayout::PendingWal);
    // Another process reading the file pins the WAL: the migration commits,
    // but its old frames cannot be cut away while that read is open.
    let reader = Connection::open(&path).unwrap();
    reader
        .execute_batch("BEGIN; SELECT count(*) FROM members;")
        .unwrap();
    let error = match Store::open_at(path.to_str(), NOW) {
        Ok(_) => panic!("a busy checkpoint must not count as a removed residue"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("checkpoint busy"), "{error}");
    assert!(error.contains("scrub_member_keys"), "{error}");
    reader.execute_batch("COMMIT").unwrap();
    drop(reader);
    {
        // The migration itself committed; only the clean-up is owed.
        let c = Connection::open(&path).unwrap();
        assert!(!super::super::table_has_column(&c, "members", "token").unwrap());
        let owed: i64 = c
            .query_row(
                "SELECT count(*) FROM storage_maintenance WHERE task = 'scrub_member_keys'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(owed, 1);
    }
    let store = Store::open_at(path.to_str(), NOW).expect("the next open finishes the scrub");
    let pending: i64 = store
        .conn()
        .unwrap()
        .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 0);
    drop(store);
    for (secret, tail) in fixture_secrets() {
        if allowed_copy(&secret).is_none() {
            assert_eq!(
                files_containing(&path, &tail),
                Vec::<String>::new(),
                "{secret}"
            );
        }
    }
}

/// A database this build creates has the hashed tables from the start, owes
/// no clean-up, and its admissions and invites store no key.
#[test]
fn a_new_database_is_created_hashed_and_owes_no_clean_up() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("new.db");
    let store = Store::open_at(path.to_str(), NOW).unwrap();
    {
        let c = store.conn().unwrap();
        for (table, column) in [
            ("members", "secret_hash"),
            ("invites", "secret_hash"),
            ("member_suspensions", "member_id"),
        ] {
            assert!(super::super::table_has_column(&c, table, column).unwrap());
        }
        assert!(!super::super::table_has_column(&c, "members", "token").unwrap());
        assert!(!super::super::table_has_column(&c, "invites", "token").unwrap());
        assert!(!super::super::table_has_column(&c, "invites", "member").unwrap());
        let owed: i64 = c
            .query_row("SELECT count(*) FROM storage_maintenance", [], |r| r.get(0))
            .unwrap();
        assert_eq!(owed, 0);
    }
    let key = synthetic("mb_", "fresh");
    let member = store
        .add_member(&key, "fresh", "agent", NOW, "master")
        .unwrap();
    assert_eq!(member.id, hashed_id("mbid_", &format!("member:{key}")));
    assert_eq!(member.secret_hash, secret_hash(&key));
    assert_eq!(
        member.principal_id,
        Some(hashed_id("pr_", &format!("member:{key}")))
    );
    assert_eq!(
        store.principal_for_credential(&key).map(|p| p.id),
        member.principal_id
    );
    assert_eq!(
        sql_values_containing(&store.conn().unwrap(), &key[7..]),
        Vec::<String>::new()
    );
    drop(store);
    // A second open of a new database migrates nothing.
    let schema = schema_dump(path.to_str().unwrap());
    drop(Store::open_at(path.to_str(), NOW + 1).unwrap());
    assert_eq!(schema_dump(path.to_str().unwrap()), schema);
}
