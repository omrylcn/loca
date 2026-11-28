//! The notification sending limit, through the hub: whose count a send is
//! taken from, and what the refusal says.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use protocol::{RoomSettings, SenderType};

use super::super::{
    BuildingRole, Hub, HubConfig, NotificationAudience, NotificationError, RequestAuthority,
};
use super::{test_now_ms, TEST_CLOCK_LOCK, TEST_NOW};
use crate::store::Store;

/// A realistic send time, in milliseconds since the epoch.
const T0: u64 = 1_800_000_000_000;

fn hub_on(store: Arc<Store>) -> Hub {
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
    hub
}

/// A Master the hub knows only by rank: no profile behind it (what a Master
/// session made before the Building had a Master principal resolves to).
fn master_without_profile() -> RequestAuthority {
    RequestAuthority {
        principal_id: None,
        display_name: Some("operator".into()),
        kind: Some(SenderType::User),
        building_role: Some(BuildingRole::Master),
    }
}

fn send(hub: &Hub, who: &RequestAuthority, title: &str) -> Result<(), NotificationError> {
    hub.send_notification(who, title, "", NotificationAudience::All)
        .map(|_| ())
}

/// In a Building with a Master principal, a Master send with no principal is
/// stored as "master" and one with it as the principal's id. They are one
/// Master and one count; each Sent list stays what it was.
#[test]
fn a_master_with_and_without_its_profile_shares_one_count() {
    let _clock = TEST_CLOCK_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::open_at(dir.path().join("hub-limit.db").to_str(), T0).unwrap());
    let hub = hub_on(store.clone());
    store
        .add_member("mb_HUBLIMITRECIPIENT", "ayla", "user", T0 - 1_000, "master")
        .unwrap();
    let with_profile = hub.resolve_authority(Some("MASTER"), None);
    let master_id = with_profile
        .principal_id
        .clone()
        .expect("a Master principal");
    let without = master_without_profile();

    for i in 0..3u64 {
        TEST_NOW.store(T0 + i * 1_000, Ordering::Relaxed);
        send(&hub, &without, &format!("rank {i}")).unwrap();
        send(&hub, &with_profile, &format!("profile {i}")).unwrap();
    }
    TEST_NOW.store(T0 + 10_000, Ordering::Relaxed);
    for who in [&without, &with_profile] {
        assert_eq!(
            send(&hub, who, "seventh"),
            Err(NotificationError::RateLimited {
                max: 6,
                retry_after_secs: 600 - 10,
            })
        );
    }
    // Stored under the key each was sent with, as before.
    assert_eq!(
        store
            .sent_notifications(Some("master"), None, 50)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        store
            .sent_notifications(Some(master_id.as_str()), None, 50)
            .unwrap()
            .len(),
        3
    );

    // The configured count governs, and the window still slides.
    assert_eq!(hub.set_notification_send_limit(7), 7);
    send(&hub, &with_profile, "seventh with seven").unwrap();
    assert!(send(&hub, &without, "eighth").is_err());
    TEST_NOW.store(T0 + 600_000, Ordering::Relaxed);
    send(&hub, &without, "after the first two left").unwrap();
    assert_eq!(hub.set_notification_send_limit(0), 1, "at least one");
}
