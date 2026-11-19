//! Bringing a name into being is one step: two requests racing for names
//! that fold together (two members; a member and a Smaster; a member and a
//! rename; two approvals; an approval and a direct admission or a Smaster)
//! end with exactly one of them in, the other refused the way its path
//! refuses, and neither waiting on the other for ever (`Hub::name_claims`).

use std::collections::HashSet;
use std::sync::{mpsc, Arc, Barrier};
use std::time::Duration;

use protocol::RoomSettings;

use super::super::{
    AdmitError, Approve, Hub, HubConfig, NameClaimError, ProfileRenameError, ADDRESS_WORD_RULE,
    AGENT_NAME_RULE,
};
use crate::store::{BuildingRole, Store};

fn admit(hub: &Hub, name: &str) -> bool {
    match hub.admit_member(name, "agent", "master") {
        Ok(_) => true,
        Err(AdmitError::Worn) => false,
        Err(error) => panic!("admit {name}: {error:?}"),
    }
}

fn admitted_token(hub: &Hub, name: &str) -> String {
    hub.admit_member(name, "agent", "master").unwrap().id
}

fn add_smaster(hub: &Hub, name: &str) -> bool {
    match hub.add_smaster(name) {
        Ok(_) => true,
        Err(NameClaimError::Worn) => false,
        Err(error) => panic!("smaster {name}: {error:?}"),
    }
}

fn rename_master(hub: &Hub, name: &str) -> bool {
    let master = hub.resolve_authority(Some(ROOT), None);
    match hub.rename_profile(&master, name) {
        Ok(_) => true,
        Err(ProfileRenameError::NameTaken) => false,
        Err(error) => panic!("rename to {name}: {error:?}"),
    }
}

fn approve(hub: &Hub, id: &str) -> bool {
    match hub.approve_join_request(id, "master") {
        Approve::Approved => true,
        Approve::NameTaken | Approve::NameWorn => false,
        other => panic!("approve {id}: {other:?}"),
    }
}

const ROOT: &str = "name-claims-fixture-root";
const ROUNDS: usize = 50;

fn hub_on(dir: &std::path::Path) -> Hub {
    Hub::build(
        HubConfig {
            admin_token: ROOT.into(),
            room_token: String::new(),
            require_sessions: true,
            require_invite: true,
            home_room: "iye".into(),
            reserved_room: "iye".into(),
            caretakers: HashSet::new(),
        },
        Arc::new(Store::open(dir.join("names.db").to_str()).unwrap()),
        RoomSettings::default(),
        1,
    )
}

/// Run `race` on a thread of its own and fail, instead of hanging, if the
/// two sides deadlock.
fn within_a_minute(race: impl FnOnce() + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = std::thread::spawn(move || {
        race();
        let _ = done.send(());
    });
    match finished.recv_timeout(Duration::from_secs(60)) {
        Ok(()) => runner.join().unwrap(),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // The race panicked: surface its message.
            runner.join().unwrap();
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the two requests deadlocked"),
    }
}

/// Start `left` and `right` together on two threads and return both answers.
fn together<A: Send + 'static, B: Send + 'static>(
    left: impl FnOnce() -> A + Send + 'static,
    right: impl FnOnce() -> B + Send + 'static,
) -> (A, B) {
    let barrier = Arc::new(Barrier::new(2));
    let ready = barrier.clone();
    let other = std::thread::spawn(move || {
        ready.wait();
        right()
    });
    barrier.wait();
    let mine = left();
    (mine, other.join().unwrap())
}

/// Everyone in the Building wearing a name that folds to `name`'s fold.
fn wearers(hub: &Hub, name: &str) -> Vec<String> {
    let folded = crate::store::search_fold(name);
    let mut worn: Vec<String> = hub
        .memberships()
        .into_iter()
        .map(|member| member.name)
        .chain(hub.smasters().into_iter().map(|record| record.name))
        .chain(
            hub.profiles()
                .into_iter()
                .filter(|profile| profile.role == BuildingRole::Master)
                .map(|profile| profile.display_name),
        )
        .filter(|worn| crate::store::search_fold(worn) == folded)
        .collect();
    worn.sort();
    worn
}

/// Two pending requests an older server could have filed for `left` and
/// `right`, and an admission right for each.
fn pending_pair(hub: &Hub, round: usize, left: &str, right: &str) -> (String, String) {
    let ids = (format!("jr_left_{round}"), format!("jr_right_{round}"));
    for (id, name) in [(&ids.0, left), (&ids.1, right)] {
        hub.store
            .create_join_request(id, "secret", name, "agent", hub.now())
            .unwrap();
    }
    hub.mint_admission_stock(2, 3_600_000, None);
    ids
}

#[test]
fn two_members_whose_names_fold_together_never_both_get_in() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (a, b) = (hub.clone(), hub.clone());
            let upper = format!("Bob-{round}");
            let lower = format!("bob-{round}");
            let answers = together(move || admit(&a, &upper), move || admit(&b, &lower));
            assert_eq!(
                (
                    wearers(&hub, &format!("bob-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?} {:?}",
                wearers(&hub, &format!("bob-{round}"))
            );
        }
    });
}

#[test]
fn two_requests_for_the_same_exact_name_admit_one_member() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (a, b) = (hub.clone(), hub.clone());
            let name = format!("dup-{round}");
            let (left, right) = (name.clone(), name.clone());
            let answers = together(
                move || admitted_token(&a, &left),
                move || admitted_token(&b, &right),
            );
            assert_eq!(answers.0, answers.1, "round {round}: two members");
            assert_eq!(wearers(&hub, &name), vec![name], "round {round}");
        }
    });
}

#[test]
fn a_member_and_a_smaster_whose_names_fold_together_never_both_get_in() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (a, b) = (hub.clone(), hub.clone());
            let member = format!("Ed-{round}");
            let smaster = format!("ed-{round}");
            let answers = together(
                move || admit(&a, &member),
                move || add_smaster(&b, &smaster),
            );
            assert_eq!(
                (
                    wearers(&hub, &format!("ed-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?} {:?}",
                wearers(&hub, &format!("ed-{round}"))
            );
        }
    });
}

#[test]
fn a_member_and_a_rename_whose_names_fold_together_never_both_get_in() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (a, b) = (hub.clone(), hub.clone());
            let member = format!("Fay-{round}");
            let renamed = format!("fay-{round}");
            let answers = together(
                move || admit(&a, &member),
                move || rename_master(&b, &renamed),
            );
            assert_eq!(
                (
                    wearers(&hub, &format!("fay-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?} {:?}",
                wearers(&hub, &format!("fay-{round}"))
            );
        }
    });
}

#[test]
fn two_approvals_whose_names_fold_together_admit_one_member() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (left, right) = pending_pair(
                &hub,
                round,
                &format!("Carol-{round}"),
                &format!("carol-{round}"),
            );
            let (a, b) = (hub.clone(), hub.clone());
            let answers = together(move || approve(&a, &left), move || approve(&b, &right));
            assert_eq!(
                (
                    wearers(&hub, &format!("carol-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?}"
            );
        }
    });
}

#[test]
fn an_approval_and_a_direct_admission_whose_names_fold_together_admit_one() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (request, _) = pending_pair(
                &hub,
                round,
                &format!("Gus-{round}"),
                &format!("unrelated-{round}"),
            );
            let (a, b) = (hub.clone(), hub.clone());
            let member = format!("gus-{round}");
            let answers = together(move || approve(&a, &request), move || admit(&b, &member));
            assert_eq!(
                (
                    wearers(&hub, &format!("gus-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?} {:?}",
                wearers(&hub, &format!("gus-{round}"))
            );
        }
    });
}

#[test]
fn an_approval_and_a_smaster_whose_names_fold_together_admit_one() {
    let dir = tempfile::tempdir().unwrap();
    let hub = hub_on(dir.path());
    within_a_minute(move || {
        for round in 0..ROUNDS {
            let (request, _) = pending_pair(
                &hub,
                round,
                &format!("Hal-{round}"),
                &format!("other-{round}"),
            );
            let (a, b) = (hub.clone(), hub.clone());
            let smaster = format!("hal-{round}");
            let answers = together(
                move || approve(&a, &request),
                move || add_smaster(&b, &smaster),
            );
            assert_eq!(
                (
                    wearers(&hub, &format!("hal-{round}")).len(),
                    answers.0 ^ answers.1
                ),
                (1, true),
                "round {round}: {answers:?} {:?}",
                wearers(&hub, &format!("hal-{round}"))
            );
        }
    });
}

/// The whole new-name rule is decided where the name is written. An older
/// server left a member under a name the rule refuses (`all`, a space and a
/// non-ASCII letter); asking again for that exact name finds it. In the order
/// a door used to take (its own lookup finds the member, the membership is
/// revoked, then the hub admits), the name is not admitted again: the only
/// lookup that counts is the hub's, under `name_claims`.
#[test]
fn a_name_the_rule_refuses_is_not_admitted_again_once_its_old_member_is_gone() {
    for legacy in ["all", "Ay\u{015F}e Kaya"] {
        let dir = tempfile::tempdir().unwrap();
        let seed = hub_on(dir.path())
            .admit_member("legacy-seed", "agent", "master")
            .unwrap()
            .id;
        // Model the rows an older server wrote when it admitted the name.
        let sql = rusqlite::Connection::open(dir.path().join("names.db")).unwrap();
        for table in [
            "UPDATE members SET name = ?1 WHERE name = 'legacy-seed'",
            "UPDATE principals SET display_name = ?1 WHERE display_name = 'legacy-seed'",
        ] {
            assert_eq!(sql.execute(table, [legacy]).unwrap(), 1, "{table}");
        }
        drop(sql);
        let hub = hub_on(dir.path());
        let again = hub.admit_member(legacy, "agent", "master").unwrap();
        assert_eq!(
            again.id, seed,
            "{legacy:?}: asking again found someone else"
        );

        // A door's own lookup finds the member here...
        assert!(hub.member_by_name(legacy).is_some());
        // ...the membership ends before the door admits...
        assert!(hub.revoke_member(&seed).unwrap());
        // ...and admitting the name now meets the rule.
        let admitted = hub.admit_member(legacy, "agent", "master");
        let rule = if legacy == "all" {
            ADDRESS_WORD_RULE
        } else {
            AGENT_NAME_RULE
        };
        assert!(
            matches!(admitted, Err(AdmitError::Rule(said)) if said == rule),
            "{legacy:?}: {admitted:?}"
        );
        assert!(
            hub.member_by_name(legacy).is_none(),
            "{legacy:?}: a new membership came into being"
        );
    }
}
