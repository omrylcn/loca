//! The name a NEW loca may be opened under.
//!
//! A loca's name is its id and its address: routes, `/ws?room=`, invites, the
//! Lobby's seats, and the agent kit's `DAVET_<name>` keys. Until v0.15 a new
//! name was refused only when empty, over 128 characters, or holding a
//! control character, so spaces, `/`, `#`, `@`, any letter and emoji went
//! through. Two things broke:
//!
//! - the kit keeps a loca's invite under `DAVET_<name>` with every character
//!   other than `A-Za-z0-9_` turned into `_`, so `my loca`, `my-loca` and
//!   `my_loca` share one key and overwrite each other's invite;
//! - a name with a space needs quoting in every kit command, and an agent
//!   that leaves the quotes off talks to the wrong loca or to none.
//!
//! So a new loca's name is ASCII letters, digits, `.`, `_` and `-`, starts
//! with a letter or digit, and is at most 128 bytes; a display name with
//! spaces or any letter is the loca's **title**. A new name whose kit key is
//! already another loca's is refused too. Only opening a loca (and
//! `ROOM_RENAME`, which gives a loca a new id) asks; every existing loca keeps
//! its name and works on every route as before.

use std::collections::HashMap;

use super::{Hub, Room};
use crate::store::Store;
use crate::sync::RecoverMutex;

/// The longest name, in bytes (unchanged from before the rule).
pub const MAX_LEN: usize = 128;

/// The kit key of a private conversation's id prefix (`Hub::DM_PREFIX`,
/// `bb-`), folded. A test holds the two together.
const RESERVED_KEY_PREFIX: &str = "bb_";

/// Why a new loca's name was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocaNameError {
    Empty,
    TooLong,
    /// The first character is not an ASCII letter or digit.
    BadStart(char),
    /// Characters outside `A-Za-z0-9._-`, in order, each once.
    BadChars(Vec<char>),
    /// The name's kit key is already another loca's. `existing` is that
    /// loca, for the server's own use: the answer never names it, because
    /// the caller may not be allowed to know it exists.
    KitKey {
        existing: String,
        key: String,
    },
    /// The name's kit key begins like a private conversation's id.
    Reserved,
}

const TITLE_HINT: &str = "A display name with spaces or other characters is the loca's title: \
open it under a short name (e.g. my-loca), then set the title in This Loca › Loca identity \
(PUT /rooms/:id/settings {\"title\"}).";

impl LocaNameError {
    /// 409 for a kit-key clash with an existing loca, 400 for the rest.
    pub fn is_conflict(&self) -> bool {
        matches!(self, Self::KitKey { .. })
    }

    pub fn message(&self) -> String {
        let rule = "A loca name may contain only ASCII letters, digits, '.', '_' and '-', \
                    must start with a letter or digit, and is at most 128 characters; \
                    it is the loca's address and its agents' DAVET_ key.";
        match self {
            Self::Empty => format!("invalid loca name: it is empty. {rule} {TITLE_HINT}"),
            Self::TooLong => {
                format!("invalid loca name: it is longer than 128 characters. {rule} {TITLE_HINT}")
            }
            Self::BadStart(c) => format!(
                "invalid loca name: it starts with {}. {rule} {TITLE_HINT}",
                shown(*c)
            ),
            Self::BadChars(chars) => format!(
                "invalid loca name: it contains {}. {rule} {TITLE_HINT}",
                chars
                    .iter()
                    .map(|c| shown(*c))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            // Not which loca: a group loca or a private conversation the
            // caller cannot see must not be named to them.
            Self::KitKey { key, .. } => format!(
                "loca name refused: another loca already has the agent kit key {key} (the kit \
                 turns every character other than letters, digits and '_' into '_', and on \
                 Windows it does not tell capitals from small letters), so an agent seated in \
                 both would keep one invite for the two locas and one would overwrite the \
                 other. Choose another name."
            ),
            Self::Reserved => "invalid loca name: names that begin with 'bb-', 'bb_' or 'bb.' \
                 (in any case) are kept for private conversations. Choose another name."
                .to_string(),
        }
    }
}

/// A refused character as the message names it; one that would not show
/// (a control, a line break, another kind of space) by its code point.
fn shown(c: char) -> String {
    if c == ' ' {
        "a space".to_string()
    } else if c.is_control() || c.is_whitespace() {
        format!("U+{:04X}", c as u32)
    } else {
        format!("'{c}'")
    }
}

/// The character rule alone, which needs no other loca to answer.
pub fn check_chars(name: &str) -> Result<(), LocaNameError> {
    let Some(first) = name.chars().next() else {
        return Err(LocaNameError::Empty);
    };
    if name.len() > MAX_LEN {
        return Err(LocaNameError::TooLong);
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-');
    let mut bad: Vec<char> = Vec::new();
    for c in name.chars().filter(|c| !allowed(*c)) {
        if !bad.contains(&c) {
            bad.push(c);
        }
    }
    if !bad.is_empty() {
        return Err(LocaNameError::BadChars(bad));
    }
    if !first.is_ascii_alphanumeric() {
        return Err(LocaNameError::BadStart(first));
    }
    Ok(())
}

/// The key under which the agent kit keeps a loca's invite, without the
/// `DAVET_` prefix: every character other than `A-Za-z0-9_` becomes one `_`.
/// This is the Python kit's `re.sub(r"[^A-Za-z0-9_]", "_", room)`
/// (`skill/agent-room/listen.py` `room_env_key`, `credentials.py`
/// `token_for_room`) and `setup.ps1`'s `-replace '[^A-Za-z0-9_]', '_'`.
/// Pinned to them by `skill/agent-room/tests/fixtures/kit_keys.json`.
pub fn kit_key(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// The same key as `connect.sh` writes it: `_var` is
/// `printf '%s' "$1" | tr -c 'A-Za-z0-9_' '_'`, and `tr` works on bytes, so
/// a letter outside ASCII becomes one `_` per byte of its UTF-8 form. Equal
/// to `kit_key` for every ASCII name.
pub fn kit_key_shell(name: &str) -> String {
    name.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b == b'_' {
                b as char
            } else {
                '_'
            }
        })
        .collect()
}

/// The full rule for a new loca's name against the locas that exist now:
/// the character rule, then no existing loca under the same kit key (in
/// either of the kit's two spellings).
pub fn check_new<'a>(
    name: &str,
    existing: impl IntoIterator<Item = &'a str>,
) -> Result<(), LocaNameError> {
    check_chars(name)?;
    // A new name is ASCII, so its two spellings are one. Keys are compared
    // without regard to case: on Windows the kit's environment is
    // case-insensitive, so DAVET_Proj and DAVET_proj are one variable there.
    let key = kit_key(name);
    let folded = key.to_ascii_lowercase();
    // A private conversation's id is `bb-` and a hash of its two people,
    // which anyone who can list people can compute. A new name under that
    // key is refused whether or not the conversation exists, so the answer
    // tells nobody that it does (a 409 only when it existed would).
    if folded.starts_with(RESERVED_KEY_PREFIX) {
        return Err(LocaNameError::Reserved);
    }
    for other in existing {
        if other != name
            && (kit_key(other).to_ascii_lowercase() == folded
                || kit_key_shell(other).to_ascii_lowercase() == folded)
        {
            return Err(LocaNameError::KitKey {
                existing: other.to_string(),
                key: format!("DAVET_{key}"),
            });
        }
    }
    Ok(())
}

/// `ROOM_RENAME=from:to` moves a loca to a new id, so `to` is a new name.
/// Asked only when the rename would happen (`from` exists, `to` does not),
/// so a variable left in the environment after its rename keeps starting.
pub fn check_rename(existing: &[String], from: &str, to: &str) -> Result<(), LocaNameError> {
    if !existing.iter().any(|room| room == from) || existing.iter().any(|room| room == to) {
        return Ok(());
    }
    check_new(
        to,
        existing
            .iter()
            .map(String::as_str)
            .filter(|room| *room != from),
    )
}

impl Hub {
    /// The full rule for `name` as a new loca of this Building.
    pub fn check_new_loca_name(&self, name: &str) -> Result<(), LocaNameError> {
        let rooms = self.rooms.lock_or_recover();
        check_new(name, rooms.keys().map(String::as_str))
    }

    /// The loopback sandbox (no `ADMIN_TOKEN`) still opens a loca the first
    /// time a room route, the socket or an invite names it. That is opening
    /// a loca too: refuse an id the rule refuses before it is created. A
    /// closed Building opens locas only through `POST /rooms`, and an id
    /// that already names a loca (or a sealed one) is never asked.
    pub fn refuse_implicit_loca(&self, room: &str) -> Option<LocaNameError> {
        if !self.admin_open() || self.room_exists(room) || self.is_deleted(room) {
            return None;
        }
        self.check_new_loca_name(room).err()
    }
}

/// The configured home loca (`LOCA_AGENT_ROOM`) is opened at boot when it
/// has no stored record yet. It is not held to the rule by force: a
/// Building whose home was configured before the rule, and never written to
/// (or that keeps nothing on disk), would otherwise stop starting on the
/// upgrade. A home the rule would refuse is opened and said in the log.
pub(super) fn require_home_rule(rooms: &HashMap<String, Room>, home: &str) {
    if rooms.contains_key(home) {
        return;
    }
    if let Err(error) = check_new(home, rooms.keys().map(String::as_str)) {
        tracing::warn!(
            home,
            "LOCA_AGENT_ROOM is not a name a new loca may take ({:?}); it is opened as configured. \
             Agents keep its invite under a key that another loca can share: choose a name of \
             ASCII letters, digits, '.', '_' and '-' when you can",
            error
        );
    }
}

/// `ROOM_RENAME` runs before the hub exists, so it asks the store which
/// locas there are. Read only when the variable is set.
pub fn require_rename_rule(store: &Store, from: &str, to: &str) {
    if to.eq_ignore_ascii_case(crate::hub::LOBBY_ROOM) {
        panic!("ROOM_RENAME refused: '{to}' is the Building's Lobby room");
    }
    let existing: Vec<String> = store
        .load()
        .expect("persistent room state is unreadable; refusing to start")
        .into_iter()
        .map(|snapshot| snapshot.room)
        .collect();
    if let Err(error) = check_rename(&existing, from, to) {
        panic!(
            "ROOM_RENAME refused: '{to}' cannot be a new loca id: {}",
            error.message()
        );
    }
}

/// A loca named `lobby` opened before the Lobby room existed (v0.18 and
/// earlier accepted the name) would become the Lobby on upgrade: its
/// history readable by every member, its Operator moderating the whole
/// Building. Refuse to start until it has a new id. Run after `ROOM_RENAME`,
/// so `ROOM_RENAME=lobby:<new>` is the way through.
pub fn require_no_legacy_lobby(store: &Store) {
    let found = store
        .legacy_loca_records(crate::hub::LOBBY_ROOM)
        .expect("persistent room state is unreadable; refusing to start");
    if !found.is_empty() {
        panic!(
            "a loca named 'lobby' from before the Lobby room exists (it has {}); \
             it would become the Building's Lobby and every member could read it. \
             Start once with ROOM_RENAME=lobby:<new-id> to give it a new id",
            found.join(", ")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct Case {
        name: String,
        python: String,
        shell: String,
    }

    /// The same table the kit's own test reads
    /// (`skill/agent-room/tests/test_kit_keys.py`), so the server's mapping
    /// and the kit's cannot drift apart.
    const KIT_KEYS: &str =
        include_str!("../../../../skill/agent-room/tests/fixtures/kit_keys.json");

    #[test]
    fn kit_key_is_the_kits_mapping() {
        let cases: Vec<Case> = serde_json::from_str(KIT_KEYS).unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            assert_eq!(
                kit_key(&case.name),
                case.python,
                "python key of {:?}",
                case.name
            );
            assert_eq!(
                kit_key_shell(&case.name),
                case.shell,
                "shell key of {:?}",
                case.name
            );
        }
    }

    #[test]
    fn the_character_rule() {
        for ok in [
            "general", "my-loca", "my_loca", "v0.15", "a", "9lives", "A.b-C_d",
        ] {
            assert_eq!(check_chars(ok), Ok(()), "{ok}");
        }
        assert_eq!(check_chars(&"a".repeat(128)), Ok(()));
        assert_eq!(check_chars(&"a".repeat(129)), Err(LocaNameError::TooLong));
        assert_eq!(check_chars(""), Err(LocaNameError::Empty));
        assert_eq!(
            check_chars("my loca"),
            Err(LocaNameError::BadChars(vec![' ']))
        );
        assert_eq!(
            check_chars("a/b#c@d/"),
            Err(LocaNameError::BadChars(vec!['/', '#', '@']))
        );
        assert_eq!(
            check_chars("işler"),
            Err(LocaNameError::BadChars(vec!['ş']))
        );
        assert_eq!(
            check_chars("ok🙂"),
            Err(LocaNameError::BadChars(vec!['🙂']))
        );
        assert_eq!(check_chars("-x"), Err(LocaNameError::BadStart('-')));
        assert_eq!(check_chars(".x"), Err(LocaNameError::BadStart('.')));
        assert_eq!(check_chars("_x"), Err(LocaNameError::BadStart('_')));
        assert!(LocaNameError::BadChars(vec![' '])
            .message()
            .contains("title"));
        assert_eq!(
            check_chars("a\nb"),
            Err(LocaNameError::BadChars(vec!['\n']))
        );
        assert!(LocaNameError::BadChars(vec!['\n'])
            .message()
            .contains("it contains U+000A."));
    }

    #[test]
    fn a_new_name_may_not_take_an_existing_locas_kit_key() {
        let existing = ["my-loca", "ünlü", "other"];
        assert_eq!(
            check_new("my_loca", existing),
            Err(LocaNameError::KitKey {
                existing: "my-loca".into(),
                key: "DAVET_my_loca".into()
            })
        );
        assert!(check_new("my.loca", existing).is_err());
        // An old name outside ASCII has two spellings in the kit: Python's
        // one `_` per letter and the shell's one per byte.
        assert!(check_new("x", ["x_"]).is_ok());
        assert!(check_new("a__", ["aş"]).is_err(), "shell key of aş is a__");
        assert!(check_new("a_", ["aş"]).is_err(), "python key of aş is a_");
        assert!(check_new("my-loca2", existing).is_ok());
        // The same name is the existing-loca conflict, answered elsewhere.
        assert!(check_new("other", existing).is_ok());
        let clash = LocaNameError::KitKey {
            existing: "my-loca".into(),
            key: "DAVET_my_loca".into(),
        };
        assert!(clash.is_conflict());
        assert!(
            !clash.message().contains("my-loca"),
            "the answer does not name the other loca: it may be one the caller cannot see"
        );
        // Windows keeps one variable for DAVET_Proj and DAVET_proj.
        assert!(check_new("proj", ["Proj"]).is_err());
        assert!(check_new("PROJ.x", ["proj-x"]).is_err());
    }

    #[test]
    fn a_new_name_under_a_private_conversations_key_is_refused_whether_or_not_one_exists() {
        assert_eq!(
            kit_key(Hub::DM_PREFIX).to_ascii_lowercase(),
            RESERVED_KEY_PREFIX,
            "the reserved prefix is the kit key of the conversation prefix"
        );
        let dm = Hub::dm_room_id("p1", "p2");
        let probe = dm.replacen("bb-", "bb_", 1);
        // The same answer with and without the conversation: no oracle.
        let with = check_new(&probe, [dm.as_str()]);
        let without = check_new(&probe, std::iter::empty::<&str>());
        assert_eq!(with, Err(LocaNameError::Reserved));
        assert_eq!(with, without);
        assert!(!LocaNameError::Reserved.is_conflict());
        for name in ["BB_x", "bb.x", "Bb_1", "bb_"] {
            assert_eq!(
                check_new(name, std::iter::empty::<&str>()),
                Err(LocaNameError::Reserved),
                "{name}"
            );
        }
        assert!(check_new("bbq", std::iter::empty::<&str>()).is_ok());
        assert!(check_new("abb_x", std::iter::empty::<&str>()).is_ok());
    }

    #[test]
    fn rename_asks_only_when_it_moves_a_loca() {
        let existing: Vec<String> = ["old loca", "my-loca", "taken"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(check_rename(&existing, "old loca", "new loca").is_err());
        assert!(check_rename(&existing, "old loca", "my_loca").is_err());
        assert!(check_rename(&existing, "old loca", "old-loca").is_ok());
        // Renaming a loca to its own kit key is no clash: it leaves.
        assert!(check_rename(&existing, "my-loca", "my_loca").is_ok());
        // Already renamed (from gone) or a target that exists: not a new name.
        assert!(check_rename(&existing, "gone", "new loca").is_ok());
        assert!(check_rename(&existing, "my-loca", "old loca").is_ok());
    }

    #[test]
    fn direct_message_and_care_ids_pass_the_rule() {
        assert_eq!(check_chars(&Hub::dm_room_id("p1", "p2")), Ok(()));
        assert_eq!(check_chars("iye"), Ok(()));
    }
}
