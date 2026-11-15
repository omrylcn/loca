//! The notification sending limit, at the store: counted from the stored
//! notifications in the transaction that would store the next one.

use std::sync::Arc;

use super::super::{NotificationPage, NotificationSend, SendLimit, Store};

/// A send time shaped like a real one (milliseconds since the epoch), so
/// nothing here passes because the clock is near zero.
const T0: u64 = 1_800_000_000_000;
/// The server's numbers: six in ten minutes.
const MAX: u32 = 6;
const WINDOW_MS: u64 = 10 * 60 * 1000;

fn open(path: &std::path::Path) -> Store {
    Store::open_at(path.to_str(), T0).unwrap()
}

/// Two recipients, as the server would resolve them.
fn recipients(store: &Store) -> Vec<String> {
    ["ayla", "bora"]
        .iter()
        .map(|name| {
            let key = format!("mb_{name}LIMITFIXTURE");
            store
                .add_member(&key, name, "user", T0 - 1_000, "master")
                .unwrap();
            store.principal_for_credential(&key).unwrap().id
        })
        .collect()
}

fn send_as(
    store: &Store,
    sender: &str,
    counted: &[String],
    title: &str,
    at: u64,
    to: &[String],
) -> NotificationSend {
    store
        .create_notification(
            sender,
            "Sender",
            r#"{"all":true}"#,
            title,
            "",
            at,
            to,
            SendLimit {
                senders: counted,
                max: MAX,
                window_ms: WINDOW_MS,
            },
        )
        .unwrap()
        .unwrap()
}

fn send(store: &Store, sender: &str, title: &str, at: u64, to: &[String]) -> NotificationSend {
    send_as(store, sender, &[sender.to_string()], title, at, to)
}

fn created(outcome: NotificationSend) -> bool {
    matches!(outcome, NotificationSend::Created(_))
}

/// (notification rows, receipt rows) in the whole file.
fn rows(store: &Store) -> (i64, i64) {
    let c = store.conn().unwrap();
    let n = c
        .query_row("SELECT COUNT(*) FROM notifications", [], |r| r.get(0))
        .unwrap();
    let r = c
        .query_row("SELECT COUNT(*) FROM notification_receipts", [], |r| {
            r.get(0)
        })
        .unwrap();
    (n, r)
}

/// (titles newest first, unread) for one recipient.
fn inbox(store: &Store, principal: &str) -> (Vec<String>, u64) {
    let (list, unread) = store
        .notifications_for(
            principal,
            NotificationPage {
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
    (list.into_iter().map(|n| n.title).collect(), unread)
}

#[test]
fn the_sixth_send_passes_and_the_seventh_is_refused_writing_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("limit.db"));
    let to = recipients(&store);
    for i in 0..6u64 {
        assert!(
            created(send(
                &store,
                "pr_sender",
                &format!("n{i}"),
                T0 + i * 1_000,
                &to
            )),
            "send {} of six",
            i + 1
        );
    }
    let before = rows(&store);
    let inboxes: Vec<_> = to.iter().map(|p| inbox(&store, p)).collect();
    assert_eq!(before, (6, 12));

    let seventh = send(&store, "pr_sender", "n6", T0 + 6_000, &to);
    assert!(
        matches!(seventh, NotificationSend::Limited { .. }),
        "{seventh:?}"
    );
    assert_eq!(rows(&store), before, "a refused send writes nothing");
    for (principal, was) in to.iter().zip(inboxes) {
        assert_eq!(
            inbox(&store, principal),
            was,
            "a refused send leaves {principal}'s list and unread count as they were"
        );
    }
}

#[test]
fn retry_after_is_when_the_oldest_of_the_six_leaves_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("retry.db"));
    let to = recipients(&store);
    // An old one already outside the window does not count.
    assert!(created(send(
        &store,
        "pr_sender",
        "old",
        T0 - WINDOW_MS,
        &to
    )));
    let first = T0 + 7_000;
    for i in 0..6u64 {
        assert!(created(send(
            &store,
            "pr_sender",
            &format!("n{i}"),
            first + i * 30_000,
            &to
        )));
    }
    let now = first + 5 * 30_000 + 1_234;
    assert_eq!(
        send(&store, "pr_sender", "refused", now, &to),
        NotificationSend::Limited {
            retry_after_ms: first + WINDOW_MS - now
        }
    );
}

#[test]
fn the_window_slides_one_more_fits_once_the_oldest_has_left() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("slide.db"));
    let to = recipients(&store);
    for i in 0..6u64 {
        assert!(created(send(
            &store,
            "pr_sender",
            &format!("n{i}"),
            T0 + i * 60_000,
            &to
        )));
    }
    // One millisecond before the oldest is ten minutes old: still six.
    assert_eq!(
        send(&store, "pr_sender", "early", T0 + WINDOW_MS - 1, &to),
        NotificationSend::Limited { retry_after_ms: 1 }
    );
    // At ten minutes it has left the window: exactly one more fits.
    assert!(created(send(
        &store,
        "pr_sender",
        "next",
        T0 + WINDOW_MS,
        &to
    )));
    assert!(matches!(
        send(&store, "pr_sender", "again", T0 + WINDOW_MS, &to),
        NotificationSend::Limited { retry_after_ms } if retry_after_ms == 60_000
    ));
}

#[test]
fn the_count_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restart.db");
    let to = {
        let store = open(&path);
        let to = recipients(&store);
        for i in 0..6u64 {
            assert!(created(send(
                &store,
                "pr_sender",
                &format!("n{i}"),
                T0 + i,
                &to
            )));
        }
        to
    };
    let store = Store::open_at(path.to_str(), T0 + 60_000).unwrap();
    assert!(matches!(
        send(&store, "pr_sender", "after restart", T0 + 60_000, &to),
        NotificationSend::Limited { .. }
    ));
    assert_eq!(rows(&store).0, 6);
}

#[test]
fn two_senders_do_not_share_a_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("two.db"));
    let to = recipients(&store);
    for i in 0..6u64 {
        assert!(created(send(
            &store,
            "pr_first",
            &format!("a{i}"),
            T0 + i,
            &to
        )));
    }
    for i in 0..6u64 {
        assert!(
            created(send(
                &store,
                "pr_second",
                &format!("b{i}"),
                T0 + 10 + i,
                &to
            )),
            "the second sender's send {} is its own",
            i + 1
        );
    }
    assert!(!created(send(&store, "pr_first", "a6", T0 + 20, &to)));
    assert!(!created(send(&store, "pr_second", "b6", T0 + 20, &to)));
}

/// A Master is stored as its principal or as "master"; counted over both, it
/// has one count. What is stored is unchanged.
#[test]
fn every_key_a_sender_is_counted_over_shares_one_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = open(&dir.path().join("keys.db"));
    let to = recipients(&store);
    let master = ["pr_master".to_string(), "master".to_string()];
    for i in 0..3u64 {
        assert!(created(send_as(
            &store,
            "master",
            &master,
            &format!("raw{i}"),
            T0 + i,
            &to
        )));
        assert!(created(send_as(
            &store,
            "pr_master",
            &master,
            &format!("session{i}"),
            T0 + 10 + i,
            &to
        )));
    }
    assert!(!created(send_as(
        &store,
        "pr_master",
        &master,
        "seventh",
        T0 + 20,
        &to
    )));
    assert!(!created(send_as(
        &store,
        "master",
        &master,
        "seventh",
        T0 + 20,
        &to
    )));
    // Each kept the key it was sent under.
    assert_eq!(
        store.sent_notifications("master", None, 50).unwrap().len(),
        3
    );
    assert_eq!(
        store
            .sent_notifications("pr_master", None, 50)
            .unwrap()
            .len(),
        3
    );
}

/// Many sends at once from one sender, through two connections to the same
/// file (the store's own mutex cannot serialise those): never more than six.
#[test]
fn concurrent_sends_from_one_sender_never_exceed_six() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race.db");
    let first = Arc::new(open(&path));
    let to = Arc::new(recipients(&first));
    let second = Arc::new(Store::open_at(path.to_str(), T0).unwrap());
    let barrier = Arc::new(std::sync::Barrier::new(16));
    let threads: Vec<_> = (0..16u64)
        .map(|i| {
            let store = if i % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let to = to.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                store
                    .create_notification(
                        "pr_sender",
                        "Sender",
                        r#"{"all":true}"#,
                        &format!("race{i}"),
                        "",
                        T0 + i,
                        &to,
                        SendLimit {
                            senders: &["pr_sender".to_string()],
                            max: MAX,
                            window_ms: WINDOW_MS,
                        },
                    )
                    .unwrap()
                    .unwrap()
            })
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    let passed = outcomes.iter().filter(|o| created(**o)).count();
    assert_eq!(passed, 6, "{outcomes:?}");
    assert_eq!(rows(&first), (6, 12));
}
