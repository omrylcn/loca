//! A loca's identity (PRINCIPLES "A loca's identity"): what it is for (a
//! description), its own number of seats, an icon and a colour from closed
//! lists, and a few labels. It is core, not an extension, and lives in
//! `RoomSettings` beside the extension set. Whoever may change the loca's
//! extensions changes it; everyone who may enter reads it; a direct message has none.
//! Every change is written to the loca's Journal with who made it.

use protocol::{RoomSettings, SenderType, ServerFrame, LOCA_COLORS, LOCA_ICONS};

use super::Hub;
use crate::sync::RecoverMutex;

/// A description is one short paragraph.
pub const DESCRIPTION_MAX_CHARS: usize = 280;
/// At most this many labels, each at most this long.
pub const LABELS_MAX: usize = 5;
pub const LABEL_MAX_CHARS: usize = 24;
/// Words that name a rank or a role. A label grants nothing, so it may not
/// read like one of them (compared without case, `-`/`_` read as a space).
pub const RESERVED_LABELS: &[&str] = &[
    "master",
    "smaster",
    "operator",
    "loca operator",
    "grand operator",
    "lead",
    "participant",
    "member",
    "wiki editor",
    "editor",
    "caretaker",
    "loca care",
    "admin",
    "root",
    "owner",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityError {
    /// A field that breaks its rule; the sentence says which and why.
    Invalid(String),
    /// Fewer seats than are held right now.
    SeatsHeld {
        held: usize,
        wanted: u32,
    },
    /// A direct message is conversation; it has no identity fields.
    Dm,
    /// The loca is closed (read-only) or sealed.
    NotWritable,
    Storage,
}

impl IdentityError {
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(text) => text.clone(),
            Self::SeatsHeld { held, wanted } => format!(
                "{held} seats are held in this loca right now; it cannot have fewer than that \
                 (asked for {wanted})"
            ),
            Self::Dm => "a birebir has no description, seats, icon, colour or labels".into(),
            Self::NotWritable => "this loca is closed — read-only".into(),
            Self::Storage => "could not save the loca's identity — try again".into(),
        }
    }
}

/// What a caller asks to change. `None` leaves a field as it is; an empty
/// text (or an empty label list) clears it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdentityEdit {
    pub description: Option<String>,
    pub seats: Option<u32>,
    pub icon: Option<String>,
    pub color: Option<String>,
    pub labels: Option<Vec<String>>,
}

/// A character that would break a line or hide itself: controls (line
/// breaks and tabs included), the Unicode line and paragraph separators, and
/// the invisible formatting characters (zero-width, direction overrides,
/// joiners, the byte-order mark, tag characters). Emoji variation selectors
/// are allowed (they draw nothing on their own).
fn hidden_or_breaking(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{034F}'
                | '\u{061C}'
                | '\u{115F}'
                | '\u{1160}'
                | '\u{17B4}'
                | '\u{17B5}'
                | '\u{180B}'..='\u{180F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{2028}'..='\u{202E}'
                | '\u{2060}'..='\u{206F}'
                | '\u{3164}'
                | '\u{FEFF}'
                | '\u{FFA0}'
                | '\u{FFF9}'..='\u{FFFB}'
                | '\u{1D173}'..='\u{1D17A}'
                | '\u{E0000}'..='\u{E0FFF}'
        )
}

fn plain_text(value: &str, what: &str, max: usize) -> Result<String, IdentityError> {
    let value = value.trim();
    if value.chars().any(hidden_or_breaking) {
        return Err(IdentityError::Invalid(format!(
            "{what} must be plain text on one line: no line breaks, control or invisible \
             formatting characters"
        )));
    }
    if value.chars().count() > max {
        return Err(IdentityError::Invalid(format!(
            "{what} is too long: at most {max} characters"
        )));
    }
    Ok(value.to_string())
}

fn reserved_label(label: &str) -> bool {
    let folded: String = label
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' || c == '_' { ' ' } else { c })
        .collect();
    let folded = folded.split_whitespace().collect::<Vec<_>>().join(" ");
    RESERVED_LABELS.contains(&folded.as_str())
}

/// The Building's two numbers of seats: what a loca without a number of its
/// own has (`LOCA_CAPACITY`), and the most a loca's own number may reach
/// (`LOCA_CAPACITY_MAX`, never below the default). Both are the
/// deployment's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatNumbers {
    pub default: usize,
    pub ceiling: usize,
}

/// Check every asked field and put it in its stored form: trimmed text,
/// labels deduplicated without regard to case (the first spelling stays).
/// `ceiling` is the most seats a loca's own number may reach.
pub fn validated(edit: &IdentityEdit, ceiling: usize) -> Result<IdentityEdit, IdentityError> {
    let description = edit
        .description
        .as_deref()
        .map(|text| plain_text(text, "the description", DESCRIPTION_MAX_CHARS))
        .transpose()?;
    if let Some(seats) = edit.seats {
        let min = Hub::LOCA_CAPACITY_MIN;
        if (seats as usize) < min || (seats as usize) > ceiling {
            return Err(IdentityError::Invalid(format!(
                "seats must be between {min} and {ceiling} (this Building's ceiling, \
                 LOCA_CAPACITY_MAX)"
            )));
        }
    }
    let icon = match edit.icon.as_deref().map(str::trim) {
        Some(icon) if !icon.is_empty() && !LOCA_ICONS.contains(&icon) => {
            return Err(IdentityError::Invalid(format!(
                "unknown loca icon: {icon} (one of: {})",
                LOCA_ICONS.join(", ")
            )))
        }
        other => other.map(str::to_string),
    };
    let color = match edit.color.as_deref().map(str::trim) {
        Some(color) if !color.is_empty() && !LOCA_COLORS.contains(&color) => {
            return Err(IdentityError::Invalid(format!(
                "unknown loca colour: {color} (one of: {})",
                LOCA_COLORS.join(", ")
            )))
        }
        other => other.map(str::to_string),
    };
    let labels = match &edit.labels {
        None => None,
        Some(asked) => {
            let mut kept: Vec<String> = Vec::new();
            for label in asked {
                let label = plain_text(label, "a label", LABEL_MAX_CHARS)?;
                if label.is_empty() {
                    return Err(IdentityError::Invalid("a label cannot be empty".into()));
                }
                if reserved_label(&label) {
                    return Err(IdentityError::Invalid(format!(
                        "\"{label}\" names a rank or a role; a label grants nothing, so it \
                         cannot be one"
                    )));
                }
                if !kept
                    .iter()
                    .any(|have| have.to_lowercase() == label.to_lowercase())
                {
                    kept.push(label);
                }
            }
            if kept.len() > LABELS_MAX {
                return Err(IdentityError::Invalid(format!(
                    "too many labels: at most {LABELS_MAX}"
                )));
            }
            Some(kept)
        }
    };
    Ok(IdentityEdit {
        description,
        seats: edit.seats,
        icon,
        color,
        labels,
    })
}

/// Write an already validated edit into `settings`.
pub fn apply(settings: &mut RoomSettings, edit: &IdentityEdit) {
    let text = |value: &Option<String>| value.clone().filter(|v| !v.is_empty());
    if edit.description.is_some() {
        settings.description = text(&edit.description);
    }
    if edit.seats.is_some() {
        settings.seats = edit.seats;
    }
    if edit.icon.is_some() {
        settings.icon = text(&edit.icon);
    }
    if edit.color.is_some() {
        settings.color = text(&edit.color);
    }
    if let Some(labels) = &edit.labels {
        settings.labels = labels.clone();
    }
}

/// The seats a loca with these settings has: its own number, held under the
/// Building's ceiling (a ceiling lowered after the number was chosen wins),
/// or the Building's default when it has none.
pub fn seats_of(settings: &RoomSettings, numbers: SeatNumbers) -> usize {
    settings
        .seats
        .map(|own| {
            (own as usize).clamp(
                Hub::LOCA_CAPACITY_MIN,
                numbers.ceiling.max(Hub::LOCA_CAPACITY_MIN),
            )
        })
        .unwrap_or(numbers.default)
}

/// Settings as they are shown (answers and `settings` frames): a loca's own
/// number of seats is the number in force. The stored number is kept, so a
/// ceiling raised again gives it back.
pub fn shown(mut settings: RoomSettings, numbers: SeatNumbers) -> RoomSettings {
    if settings.seats.is_some() {
        settings.seats = Some(seats_of(&settings, numbers) as u32);
    }
    settings
}

/// The Journal line for a change, e.g. `identity: description set; seats 7 → 12`.
/// `None` when nothing changed.
pub fn change_line(
    before: &RoomSettings,
    after: &RoomSettings,
    numbers: SeatNumbers,
) -> Option<String> {
    let mut parts = Vec::new();
    match (&before.description, &after.description) {
        (None, Some(_)) => parts.push("description set".to_string()),
        (Some(_), None) => parts.push("description removed".to_string()),
        (Some(a), Some(b)) if a != b => parts.push("description changed".to_string()),
        _ => {}
    }
    if before.seats != after.seats {
        parts.push(format!(
            "seats {} → {}",
            seats_of(before, numbers),
            seats_of(after, numbers)
        ));
    }
    if before.icon != after.icon {
        parts.push(match &after.icon {
            Some(icon) => format!("icon {icon}"),
            None => "icon removed".into(),
        });
    }
    if before.color != after.color {
        parts.push(match &after.color {
            Some(color) => format!("colour {color}"),
            None => "colour removed".into(),
        });
    }
    if before.labels != after.labels {
        parts.push(if after.labels.is_empty() {
            "labels removed".into()
        } else {
            format!("labels {}", after.labels.join(", "))
        });
    }
    (!parts.is_empty()).then(|| format!("identity: {}", parts.join("; ")))
}

/// The Journal line for a loca opened with identity fields. `None` when it
/// was opened without any.
pub fn opened_line(settings: &RoomSettings, numbers: SeatNumbers) -> Option<String> {
    let mut parts = Vec::new();
    if settings.description.is_some() {
        parts.push("description".to_string());
    }
    if settings.seats.is_some() {
        parts.push(format!("{} seats", seats_of(settings, numbers)));
    }
    if let Some(icon) = &settings.icon {
        parts.push(format!("icon {icon}"));
    }
    if let Some(color) = &settings.color {
        parts.push(format!("colour {color}"));
    }
    if !settings.labels.is_empty() {
        parts.push(format!("labels {}", settings.labels.join(", ")));
    }
    (!parts.is_empty()).then(|| format!("identity: opened with {}", parts.join("; ")))
}

impl Hub {
    /// The most seats a loca's own number may reach (`LOCA_CAPACITY_MAX`),
    /// never below the default (`LOCA_CAPACITY`).
    pub fn loca_capacity_max(&self) -> usize {
        self.loca_capacity_max
            .load(std::sync::atomic::Ordering::Relaxed)
            .clamp(self.loca_capacity(), Self::LOCA_CAPACITY_MAX)
    }

    /// Configure the ceiling, held to `LOCA_CAPACITY..=LOCA_CAPACITY_MAX`
    /// (32). Returns the value in force. A loca whose own number is above a
    /// lowered ceiling has the ceiling's seats; its number is kept.
    pub fn set_loca_capacity_max(&self, seats: usize) -> usize {
        self.loca_capacity_max
            .store(seats, std::sync::atomic::Ordering::Relaxed);
        self.loca_capacity_max()
    }

    pub fn seat_numbers(&self) -> SeatNumbers {
        SeatNumbers {
            default: self.loca_capacity(),
            ceiling: self.loca_capacity_max(),
        }
    }

    /// Settings as answers and frames show them (see [`shown`]).
    pub fn shown_settings(&self, settings: RoomSettings) -> RoomSettings {
        shown(settings, self.seat_numbers())
    }

    /// Seats in `room`: its own number (under the ceiling), or the
    /// Building's default.
    pub fn room_capacity(&self, room: &str) -> usize {
        let numbers = self.seat_numbers();
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| seats_of(&r.settings, numbers))
            .unwrap_or(numbers.default)
    }

    /// Live invites for `room`. Call it under the invite lock when the answer
    /// decides a write.
    pub(super) fn invite_count(&self, room: &str) -> usize {
        self.invites
            .lock_or_recover()
            .values()
            .filter(|invite| invite.room == room)
            .count()
    }

    /// Change the loca's identity. The caller has already checked authority
    /// (the same check as for its extensions). Seats never go below what is
    /// held now: the live invites (an invite IS a seat) and the people sitting
    /// at the table, whichever is more. The setting and its Journal line are
    /// one write.
    pub fn edit_identity(
        &self,
        room: &str,
        edit: &IdentityEdit,
        by: &str,
        by_type: SenderType,
    ) -> Result<RoomSettings, IdentityError> {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(IdentityError::Dm);
        }
        let numbers = self.seat_numbers();
        let edit = validated(edit, numbers.ceiling)?;
        if !self.is_writable(room) {
            return Err(IdentityError::NotWritable);
        }
        // Under the invite lock: no invite is minted between counting the
        // seats held and lowering the number (the mint counts under it too).
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let invites = self.invite_count(room);
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        if let Some(wanted) = edit.seats {
            let held = invites.max(r.members.len());
            if (wanted as usize) < held {
                return Err(IdentityError::SeatsHeld { held, wanted });
            }
        }
        let before = r.settings.clone();
        let mut s = before.clone();
        apply(&mut s, &edit);
        let Some(text) = change_line(&before, &s, numbers) else {
            return Ok(shown(s, numbers));
        };
        let entry = protocol::JournalEntry {
            id: r.next_journal_id,
            room: room.to_string(),
            by: by.to_string(),
            by_type,
            text,
            at: (self.now_ms)(),
        };
        self.store
            .save_room_with_journal(room, &r.mode, &s, &entry)
            .map_err(|_| IdentityError::Storage)?;
        r.settings = s.clone();
        let s = shown(s, numbers);
        let _ = r.tx.send(ServerFrame::Settings {
            settings: s.clone(),
        });
        r.next_journal_id = entry.id + 1;
        r.journal.push(entry.clone());
        // Written always ("Changes are known"); pushed only while the
        // Journal is on, like an extension change.
        if r.settings.extension_enabled("journal") {
            let _ = r.tx.send(ServerFrame::Journal { entry });
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit() -> IdentityEdit {
        IdentityEdit::default()
    }

    #[test]
    fn description_is_one_plain_bounded_paragraph() {
        let ok = validated(
            &IdentityEdit {
                description: Some("  Release 14: ship the sync fix  ".into()),
                ..edit()
            },
            7,
        )
        .unwrap();
        assert_eq!(
            ok.description.as_deref(),
            Some("Release 14: ship the sync fix")
        );
        for bad in [
            "two\nlines",
            "carriage\rreturn",
            "a\ttab",
            "line\u{2028}separator",
            "zero\u{200B}width",
            "right\u{202E}to left",
            "isolate\u{2066}x",
            "bom\u{FEFF}",
            "tag\u{E0041}",
            "soft\u{00AD}hyphen",
        ] {
            let error = validated(
                &IdentityEdit {
                    description: Some(bad.into()),
                    ..edit()
                },
                7,
            )
            .unwrap_err();
            assert!(
                error.message().contains("plain text on one line"),
                "{bad:?}"
            );
        }
        let long = "é".repeat(DESCRIPTION_MAX_CHARS + 1);
        assert!(validated(
            &IdentityEdit {
                description: Some(long),
                ..edit()
            },
            7
        )
        .unwrap_err()
        .message()
        .contains("at most 280"));
        // Exactly the cap, counted in characters, not bytes.
        assert!(validated(
            &IdentityEdit {
                description: Some("é".repeat(DESCRIPTION_MAX_CHARS)),
                ..edit()
            },
            7
        )
        .is_ok());
    }

    #[test]
    fn seats_stay_between_two_and_the_buildings_number() {
        for (seats, ceiling, ok) in [
            (2, 7, true),
            (7, 7, true),
            (12, 16, true),
            (1, 7, false),
            (0, 7, false),
            (8, 7, false),
            (17, 16, false),
        ] {
            let result = validated(
                &IdentityEdit {
                    seats: Some(seats),
                    ..edit()
                },
                ceiling,
            );
            assert_eq!(result.is_ok(), ok, "{seats} of {ceiling}: {result:?}");
            if let Err(error) = result {
                assert!(error
                    .message()
                    .contains(&format!("between 2 and {ceiling}")));
            }
        }
    }

    #[test]
    fn icon_and_colour_come_from_closed_lists() {
        assert!(validated(
            &IdentityEdit {
                icon: Some("rocket".into()),
                color: Some("sky".into()),
                ..edit()
            },
            7
        )
        .is_ok());
        for icon in ["Rocket", "dot", "https://x/y.svg", "<svg/>", "star"] {
            assert!(
                validated(
                    &IdentityEdit {
                        icon: Some(icon.into()),
                        ..edit()
                    },
                    7
                )
                .is_err(),
                "{icon}"
            );
        }
        for color in ["#ff0000", "red", "rgb(0,0,0)", "teal", "SKY"] {
            assert!(
                validated(
                    &IdentityEdit {
                        color: Some(color.into()),
                        ..edit()
                    },
                    7
                )
                .is_err(),
                "{color}"
            );
        }
        // Empty clears.
        let cleared = validated(
            &IdentityEdit {
                icon: Some(String::new()),
                color: Some(String::new()),
                ..edit()
            },
            7,
        )
        .unwrap();
        let mut s = RoomSettings {
            icon: Some("book".into()),
            color: Some("green".into()),
            ..RoomSettings::default()
        };
        apply(&mut s, &cleared);
        assert_eq!((s.icon, s.color), (None, None));
        // The person icons are not loca icons.
        for person in [
            "dot", "ring", "square", "diamond", "triangle", "hexagon", "star", "spark", "leaf",
            "wave", "moon", "sun",
        ] {
            assert!(!LOCA_ICONS.contains(&person), "{person}");
        }
        assert_eq!(LOCA_ICONS.len(), 24);
    }

    #[test]
    fn labels_are_few_short_plain_deduplicated_and_never_a_rank() {
        let ok = validated(
            &IdentityEdit {
                labels: Some(vec![" backend ".into(), "Release".into(), "BACKEND".into()]),
                ..edit()
            },
            7,
        )
        .unwrap();
        assert_eq!(
            ok.labels,
            Some(vec!["backend".to_string(), "Release".into()])
        );
        let refused = |labels: Vec<&str>| {
            validated(
                &IdentityEdit {
                    labels: Some(labels.into_iter().map(String::from).collect()),
                    ..edit()
                },
                7,
            )
            .unwrap_err()
            .message()
        };
        assert!(refused(vec!["a", "b", "c", "d", "e", "f"]).contains("at most 5"));
        // Six spellings of five labels are five labels.
        assert!(validated(
            &IdentityEdit {
                labels: Some(["a", "b", "c", "d", "e", "E"].map(String::from).to_vec()),
                ..edit()
            },
            7
        )
        .is_ok());
        assert!(refused(vec![&"x".repeat(LABEL_MAX_CHARS + 1)]).contains("at most 24"));
        assert!(refused(vec!["   "]).contains("cannot be empty"));
        assert!(refused(vec!["two\nlines"]).contains("plain text"));
        assert!(refused(vec!["a\u{200D}b"]).contains("plain text"));
        for rank in [
            "MASTER",
            "Smaster",
            "operator",
            "Lead",
            "wiki-editor",
            "Wiki_Editor",
            "loca  care",
            "participant",
        ] {
            assert!(refused(vec![rank]).contains("grants nothing"), "{rank}");
        }
        // A word that merely contains one is fine.
        assert!(validated(
            &IdentityEdit {
                labels: Some(vec!["leadership-forum".into(), "mastering".into()]),
                ..edit()
            },
            7
        )
        .is_ok());
    }

    const N: SeatNumbers = SeatNumbers {
        default: 7,
        ceiling: 32,
    };

    #[test]
    fn journal_lines_say_what_changed() {
        let before = RoomSettings::default();
        let after = RoomSettings {
            description: Some("x".into()),
            seats: Some(12),
            icon: Some("rocket".into()),
            color: Some("blue".into()),
            labels: vec!["backend".into(), "q4".into()],
            ..RoomSettings::default()
        };
        assert_eq!(
            change_line(&before, &after, N).unwrap(),
            "identity: description set; seats 7 → 12; icon rocket; colour blue; labels backend, q4"
        );
        assert_eq!(
            opened_line(&after, N).unwrap(),
            "identity: opened with description; 12 seats; icon rocket; colour blue; labels backend, q4"
        );
        assert_eq!(
            change_line(&after, &before, N).unwrap(),
            "identity: description removed; seats 12 → 7; icon removed; colour removed; labels removed"
        );
        assert_eq!(change_line(&after, &after, N), None);
        assert_eq!(opened_line(&before, N), None);
    }

    #[test]
    fn a_loca_without_its_own_number_has_the_buildings() {
        let numbers = |default, ceiling| SeatNumbers { default, ceiling };
        let plain = RoomSettings::default();
        assert_eq!(seats_of(&plain, numbers(7, 32)), 7);
        assert_eq!(seats_of(&plain, numbers(12, 32)), 12);
        let own = RoomSettings {
            seats: Some(15),
            ..RoomSettings::default()
        };
        // Above the default, within the ceiling: its own number.
        assert_eq!(seats_of(&own, numbers(7, 32)), 15);
        // The deployment lowered the ceiling under the loca's number: the
        // ceiling holds, and the answer shows the number in force while the
        // stored number is kept for a ceiling raised again.
        assert_eq!(seats_of(&own, numbers(7, 10)), 10);
        assert_eq!(shown(own.clone(), numbers(7, 10)).seats, Some(10));
        assert_eq!(shown(own.clone(), numbers(7, 32)).seats, Some(15));
        assert_eq!(own.seats, Some(15));
        assert_eq!(shown(plain.clone(), numbers(7, 10)).seats, None);
        // An older stored shape reads with no identity at all.
        let parsed: RoomSettings =
            serde_json::from_str(r#"{"rate_limit":10,"rate_window_secs":30}"#).unwrap();
        assert_eq!(parsed, RoomSettings::default());
        assert!(!serde_json::to_string(&parsed).unwrap().contains("seats"));
    }
}
