//! The agent kit version a live connection says it runs.
//!
//! A kit sends the content of its `VERSION` file as `?kit_version=` when it
//! opens its loca socket and its Lobby socket. The value is display only: it
//! is checked against a short, closed shape, kept for exactly as long as that
//! connection lives (never in the database), and shown as `kit_version` on the
//! seat's roster `Member` and on the Building list entry of the connected
//! member. Nothing reads it for authority, routing or attribution; a value of
//! any other shape is dropped at the door and stored nowhere. A browser sends
//! none, and an older kit sends none: its seat simply carries no version.

use std::collections::HashMap;
use std::fmt;

use super::{Hub, Room};
use crate::sync::RecoverMutex;

/// The query key a kit sends its version under.
pub const KIT_VERSION_QUERY: &str = "kit_version";

/// The longest version accepted, in bytes.
const MAX_LEN: usize = 32;
/// The most digits one numeric part may have.
const MAX_PART_DIGITS: usize = 4;
/// The longest pre-release suffix accepted after `-`.
const MAX_PRE_RELEASE: usize = 16;

/// A kit version that passed [`KitVersion::parse`]: `MAJOR.MINOR.PATCH`,
/// each part 0 or a number without a leading zero and of at most four
/// digits, optionally followed by `-` and a short pre-release of ASCII
/// letters, digits, `.` and `-`. At most 32 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KitVersion(String);

impl KitVersion {
    /// The value, when it has exactly the accepted shape; `None` otherwise.
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() || raw.len() > MAX_LEN || !raw.is_ascii() {
            return None;
        }
        let (core, pre) = match raw.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (raw, None),
        };
        let parts: Vec<&str> = core.split('.').collect();
        if parts.len() != 3 || !parts.iter().all(|part| numeric_part(part)) {
            return None;
        }
        if let Some(pre) = pre {
            let allowed = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
            if pre.is_empty() || pre.len() > MAX_PRE_RELEASE || !pre.chars().all(allowed) {
                return None;
            }
        }
        Some(Self(raw.to_string()))
    }

    /// The `MAJOR.MINOR.PATCH` numbers, for ordering only.
    fn core(&self) -> (u32, u32, u32) {
        let core = self.0.split('-').next().unwrap_or_default();
        let mut parts = core.split('.').map(|part| part.parse::<u32>().unwrap_or(0));
        (
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
            parts.next().unwrap_or(0),
        )
    }
}

impl fmt::Display for KitVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

fn numeric_part(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= MAX_PART_DIGITS
        && part.bytes().all(|b| b.is_ascii_digit())
        && (part == "0" || !part.starts_with('0'))
}

/// The version a connection's query string carries, when it is one.
pub fn from_query(query: &HashMap<String, String>) -> Option<KitVersion> {
    query
        .get(KIT_VERSION_QUERY)
        .and_then(|raw| KitVersion::parse(raw))
}

/// Of several versions one member runs at once (a Lobby socket and seats),
/// the oldest: that is the one that may lack something.
fn oldest<'a>(versions: impl Iterator<Item = &'a KitVersion>) -> Option<&'a KitVersion> {
    versions.min_by_key(|version| version.core())
}

/// Live Lobby connections that sent a version: connection id -> (the
/// membership's id, its version).
pub(crate) type LobbyKits = HashMap<u64, (String, KitVersion)>;

impl Room {
    /// The version of the seat `identity`, while the connection that sent it
    /// is one of the seat's live connections.
    pub(super) fn seat_kit_version(&self, identity: &str) -> Option<&KitVersion> {
        let (session, version) = self.seat_kits.get(identity)?;
        let (_, _, live) = self.members.get(identity)?;
        live.contains(session).then_some(version)
    }
}

/// Forgets a connection's version when the connection ends, whichever way
/// its session function returns.
pub struct KitVersionGuard {
    hub: Hub,
    place: Place,
    connection: u64,
}

enum Place {
    Seat { room: String, identity: String },
    Lobby,
}

impl Drop for KitVersionGuard {
    fn drop(&mut self) {
        match &self.place {
            Place::Seat { room, identity } => {
                let mut rooms = self.hub.rooms.lock_or_recover();
                if let Some(r) = rooms.get_mut(room) {
                    if r.seat_kits.get(identity).map(|(s, _)| *s) == Some(self.connection) {
                        r.seat_kits.remove(identity);
                    }
                }
            }
            Place::Lobby => {
                self.hub
                    .lobby_kits
                    .lock_or_recover()
                    .remove(&self.connection);
            }
        }
    }
}

impl Hub {
    /// Record the version the loca connection `session` sent for the seat
    /// `identity`. Call it before `join`, so the roster `join` broadcasts
    /// already carries it; it shows only while `session` holds the seat.
    /// `None` (a browser, an older kit) records nothing.
    pub fn note_seat_kit_version(
        &self,
        room: &str,
        identity: &str,
        session: u64,
        version: Option<KitVersion>,
    ) -> Option<KitVersionGuard> {
        let version = version?;
        // A sealed loca is not brought back by recording a version; `join`
        // refuses the seat there anyway.
        if self.is_deleted(room) {
            return None;
        }
        {
            let mut rooms = self.rooms.lock_or_recover();
            let r = Self::room_mut(&mut rooms, room, &self.default_settings);
            r.seat_kits.insert(identity.to_string(), (session, version));
        }
        Some(KitVersionGuard {
            hub: self.clone(),
            place: Place::Seat {
                room: room.to_string(),
                identity: identity.to_string(),
            },
            connection: session,
        })
    }

    /// Record the version a Lobby connection of membership `member_id` sent.
    pub fn note_lobby_kit_version(
        &self,
        member_id: &str,
        version: Option<KitVersion>,
    ) -> Option<KitVersionGuard> {
        let version = version?;
        let connection = self.next_session_id();
        self.lobby_kits
            .lock_or_recover()
            .insert(connection, (member_id.to_string(), version));
        Some(KitVersionGuard {
            hub: self.clone(),
            place: Place::Lobby,
            connection,
        })
    }

    /// Fill `kit_version` on each Building list entry from the member's live
    /// connections: its Lobby sockets (by membership id) and the seats it
    /// holds (by name, as `residents` lights `online`). The oldest wins.
    pub(super) fn add_kit_versions(&self, residents: &mut [protocol::Resident]) {
        let mut by_name: HashMap<String, Vec<KitVersion>> = HashMap::new();
        {
            let rooms = self.rooms.lock_or_recover();
            for r in rooms.values() {
                for (identity, (name, _, _)) in &r.members {
                    if let Some(version) = r.seat_kit_version(identity) {
                        by_name
                            .entry(name.clone())
                            .or_default()
                            .push(version.clone());
                    }
                }
            }
        }
        let lobby = self.lobby_kits.lock_or_recover();
        for resident in residents.iter_mut() {
            let from_lobby = lobby
                .values()
                .filter(|(member, _)| Some(member) == resident.membership_id.as_ref())
                .map(|(_, version)| version);
            let from_seats = by_name.get(&resident.name).into_iter().flatten();
            resident.kit_version = oldest(from_lobby.chain(from_seats)).map(KitVersion::to_string);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KitVersion;

    #[test]
    fn accepts_only_a_short_semver_shape() {
        for good in [
            "0.16.0",
            "1.2.3",
            "10.0.9999",
            "0.17.0-rc.1",
            "2.0.0-beta-2",
        ] {
            assert_eq!(
                KitVersion::parse(good).map(|v| v.to_string()),
                Some(good.to_string()),
                "{good}"
            );
        }
        for bad in [
            "",
            "0.16",
            "0.16.0.1",
            "v0.16.0",
            "0.16.0 ",
            " 0.16.0",
            "0.16.0\n",
            "0.16.0\u{7}",
            "0.1\u{0}6.0",
            "00.16.0",
            "0.016.0",
            "0.16.10000",
            "0.16.0-",
            "0.16.0+build",
            "0.16.0-rc_1",
            "0.16.0-0123456789abcdefg",
            "0.16.0-é",
            "1.2.3; drop table",
            "<script>",
            "x.y.z",
            &"9".repeat(40),
        ] {
            assert_eq!(KitVersion::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_oldest_of_several_is_chosen() {
        let versions: Vec<KitVersion> = ["0.16.0", "0.9.12", "0.15.3-rc.1"]
            .iter()
            .filter_map(|v| KitVersion::parse(v))
            .collect();
        assert_eq!(
            super::oldest(versions.iter()).map(KitVersion::to_string),
            Some("0.9.12".to_string())
        );
    }
}
