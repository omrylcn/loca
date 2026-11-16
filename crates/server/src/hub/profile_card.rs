//! A person's optional full name and profile icon: what they are shown as,
//! never who they are. The `@name` stays the one unique, ASCII, addressable
//! name; nothing here is read for authority, routing, deduplication or
//! attribution. See `docs/concepts.md` "Full name and profile icon".

use std::collections::HashMap;

use protocol::{Member, ServerFrame};

use super::{is_address_word, Hub, RESERVED_NAMES};
use crate::store::{CardPatch, ProfileCard, ProfileCardError};
use crate::sync::RecoverMutex;

/// The profile icons, drawn by the clients in the app's own line style. A
/// closed list: no uploads, no URLs, no emoji.
/// Values are only ever added: a stored icon keeps meaning what it meant.
/// None is a loca's icon (`protocol::LOCA_ICONS`), so a person and a loca
/// are never drawn alike.
/// `initials` is drawn from the person's own full name (or `@name`).
pub const PROFILE_ICONS: &[&str] = &[
    "dot",
    "ring",
    "square",
    "diamond",
    "triangle",
    "hexagon",
    "star",
    "spark",
    "leaf",
    "wave",
    "moon",
    "sun",
    "initials",
    "laptop",
    "glasses",
    "coffee",
    "headphones",
    "umbrella",
    "bicycle",
    "cat",
    "fox",
    "owl",
    "bird",
    "fish",
    "turtle",
    "mountain",
    "tree",
    "flower",
    "planet",
    "bolt",
    "drop",
];

/// The avatar's background colours, by name; each client draws its own
/// shade. Absent is "no colour": the icon alone, in the text colour.
pub const AVATAR_COLORS: &[&str] = &[
    "teal", "green", "lime", "sky", "blue", "indigo", "violet", "plum", "rose", "red", "orange",
    "amber", "sand", "slate",
];

/// The avatar's outline. Absent reads as `circle`.
pub const AVATAR_SHAPES: &[&str] = &["circle", "rounded", "square", "hexagon"];

/// Longest full name, in characters (Unicode scalar values) after trimming.
pub const FULL_NAME_MAX_CHARS: usize = 64;

/// Punctuation personal names use: apostrophes, hyphens, the full stop and
/// the two middle dots (Catalan `l·l`, Japanese `・`).
fn name_punctuation(c: char) -> bool {
    matches!(
        c,
        '\'' | '\u{2019}' | '-' | '\u{2010}' | '.' | '\u{00B7}' | '\u{30FB}'
    )
}

/// Characters that change direction or are not seen: they could make one
/// string look like another, or move the text around it.
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200D}'
            | '\u{2060}'..='\u{2064}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

fn line_break(c: char) -> bool {
    matches!(
        c,
        '\n' | '\r' | '\u{000B}' | '\u{000C}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

/// A combining accent from the basic block, for names typed decomposed
/// (`O` + `\u{308}`). At most two may follow one letter.
fn combining_accent(c: char) -> bool {
    matches!(c, '\u{0300}'..='\u{036F}') && c != '\u{034F}'
}

/// Validate a full name. `Ok(None)` clears it (empty, or only spaces).
///
/// Allowed: Unicode letters, combining accents (two per letter at most),
/// single spaces between words, and `' ’ - ‐ . · ・`; at least one letter;
/// at most `FULL_NAME_MAX_CHARS` characters. Runs of spaces become one.
/// Refused, each with its own reason: line breaks and control characters,
/// direction controls, invisible characters, `@` (a full name is never an
/// address), digits, symbols and emoji, and a name that reads as a rank or
/// title word (`RESERVED_NAMES`, `@all`) once case, spaces and punctuation
/// are set aside, because those words are what the clients print as badges.
pub(crate) fn validate_full_name(raw: &str) -> Result<Option<String>, String> {
    let mut name = String::new();
    let mut letters = 0usize;
    let mut accents_in_a_row = 0usize;
    let mut previous_letter = false;
    for c in raw.chars() {
        if line_break(c) {
            return Err("a full name is one line: line breaks are not allowed".into());
        }
        if c.is_control() {
            return Err("a full name holds no control characters".into());
        }
        if bidi_control(c) {
            return Err("a full name holds no direction-control characters".into());
        }
        if invisible(c) {
            return Err("a full name holds no invisible characters".into());
        }
        if c == '@' {
            return Err("a full name is not an address: '@' is not allowed".into());
        }
        if c == ' ' {
            if !name.is_empty() && !name.ends_with(' ') {
                name.push(' ');
            }
            accents_in_a_row = 0;
            previous_letter = false;
            continue;
        }
        if combining_accent(c) {
            accents_in_a_row += 1;
            if !previous_letter || accents_in_a_row > 2 {
                return Err("an accent must follow a letter, two at most".into());
            }
            name.push(c);
            continue;
        }
        accents_in_a_row = 0;
        if c.is_alphabetic() {
            letters += 1;
            previous_letter = true;
            name.push(c);
        } else if name_punctuation(c) {
            previous_letter = false;
            name.push(c);
        } else if c.is_whitespace() {
            return Err("a full name uses plain spaces only".into());
        } else {
            return Err(
                "a full name holds letters, spaces and ' - . only (no digits, symbols or emoji)"
                    .into(),
            );
        }
    }
    let name = name.trim_end().to_string();
    if name.is_empty() {
        return Ok(None);
    }
    if letters == 0 {
        return Err("a full name needs at least one letter".into());
    }
    if name.chars().count() > FULL_NAME_MAX_CHARS {
        return Err(format!(
            "a full name is at most {FULL_NAME_MAX_CHARS} characters"
        ));
    }
    let word: String = name.chars().filter(|c| c.is_alphabetic()).collect();
    let folded = crate::store::search_fold(&word);
    if RESERVED_NAMES.contains(&folded.as_str()) || is_address_word(&folded) {
        return Err(
            "that word is a rank or title, not a full name — use the person's own name".into(),
        );
    }
    Ok(Some(name))
}

/// Validate an icon. `Ok(None)` clears it.
pub(crate) fn validate_icon(raw: &str) -> Result<Option<String>, String> {
    if raw.is_empty() {
        return Ok(None);
    }
    if PROFILE_ICONS.contains(&raw) {
        return Ok(Some(raw.to_string()));
    }
    Err(format!(
        "`icon` must be one of: {}",
        PROFILE_ICONS.join(", ")
    ))
}

/// Validate a value from one of the avatar's closed lists. `Ok(None)` clears it.
fn validate_choice(key: &str, allowed: &[&str], raw: &str) -> Result<Option<String>, String> {
    if raw.is_empty() {
        return Ok(None);
    }
    if allowed.contains(&raw) {
        return Ok(Some(raw.to_string()));
    }
    Err(format!("`{key}` must be one of: {}", allowed.join(", ")))
}

pub(crate) fn validate_avatar_color(raw: &str) -> Result<Option<String>, String> {
    validate_choice("avatar_color", AVATAR_COLORS, raw)
}

pub(crate) fn validate_avatar_shape(raw: &str) -> Result<Option<String>, String> {
    validate_choice("avatar_shape", AVATAR_SHAPES, raw)
}

/// A field of the body: absent leaves it, `null` or `""` clears it.
fn field<'a>(
    fields: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Result<Option<&'a str>, String> {
    match fields.get(key) {
        None => Ok(None),
        Some(serde_json::Value::Null) => Ok(Some("")),
        Some(serde_json::Value::String(text)) => Ok(Some(text.as_str())),
        Some(_) => Err(format!("`{key}` must be a string or null")),
    }
}

impl Hub {
    /// The principal's card (empty when none is set).
    pub fn profile_card(&self, principal_id: &str) -> ProfileCard {
        self.store.profile_card(principal_id)
    }

    /// Every active person's card, by principal id: what the roster reads.
    pub fn profile_cards(&self) -> HashMap<String, ProfileCard> {
        self.store.profile_cards_by_principal()
    }

    /// Every active person's principal id and card by the `@name` they wear
    /// now, for the Building lists that name people (`Store::people_by_name`).
    pub fn people_by_name(&self) -> HashMap<String, (String, ProfileCard)> {
        self.store.people_by_name()
    }

    /// Record which principal the door verified for a seat, before it joins:
    /// the principal its messages are attributed to. `None` (an open door,
    /// the root key) leaves the seat with its plain name.
    pub fn prove_seat(&self, room: &str, identity: &str, principal_id: Option<&str>) {
        let mut rooms = self.rooms.lock_or_recover();
        let Some(r) = rooms.get_mut(room) else {
            return;
        };
        match principal_id {
            Some(principal) => {
                r.seat_principals
                    .insert(identity.to_string(), principal.to_string());
            }
            None => {
                r.seat_principals.remove(identity);
            }
        }
    }

    /// The caller sets its own full name and icon. The body names no person:
    /// the principal is the authenticated caller's. Any unknown field or
    /// refused value rejects the whole body and changes nothing. Every loca
    /// the person sits in is sent its roster again, which is how open
    /// clients learn the new label.
    pub fn update_profile_card(
        &self,
        principal_id: &str,
        patch: &serde_json::Value,
    ) -> Result<ProfileCard, ProfileCardError> {
        let Some(fields) = patch.as_object() else {
            return Err(ProfileCardError::Invalid(
                "the body must be a JSON object".into(),
            ));
        };
        const FIELDS: [&str; 4] = ["full_name", "icon", "avatar_color", "avatar_shape"];
        if let Some(key) = fields.keys().find(|k| !FIELDS.contains(&k.as_str())) {
            return Err(ProfileCardError::Invalid(format!(
                "unknown field `{key}`: only full_name, icon, avatar_color and avatar_shape are set here"
            )));
        }
        let checked = |key: &str, validate: fn(&str) -> Result<Option<String>, String>| {
            field(fields, key)
                .map_err(ProfileCardError::Invalid)?
                .map(validate)
                .transpose()
                .map_err(ProfileCardError::Invalid)
        };
        let full_name = checked("full_name", validate_full_name)?;
        let icon = checked("icon", validate_icon)?;
        let avatar_color = checked("avatar_color", validate_avatar_color)?;
        let avatar_shape = checked("avatar_shape", validate_avatar_shape)?;
        let card = self.store.set_profile_card(
            principal_id,
            &CardPatch {
                full_name: full_name.as_ref().map(Option::as_deref),
                icon: icon.as_ref().map(Option::as_deref),
                avatar_color: avatar_color.as_ref().map(Option::as_deref),
                avatar_shape: avatar_shape.as_ref().map(Option::as_deref),
            },
        )?;
        self.announce_profile_card(principal_id);
        Ok(card)
    }

    /// Send the roster again in every loca where this principal holds a
    /// verified seat: the frame every client already takes on each join and
    /// leave, now carrying the new card.
    fn announce_profile_card(&self, principal_id: &str) {
        let cards = self.profile_cards();
        let rooms = self.rooms.lock_or_recover();
        for room in rooms.values() {
            let seated = room.seat_principals.iter().any(|(identity, principal)| {
                principal == principal_id && room.members.contains_key(identity)
            });
            if seated {
                let members: Vec<Member> = room.member_list(&cards);
                let _ = room.tx.send(ServerFrame::Members { members });
            }
        }
    }
}
