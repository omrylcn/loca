//! Loca extensions (PRINCIPLES "Loca extensions").
//!
//! A loca has a set of enabled extensions; chat is always on and is not one.
//! The set lives in `RoomSettings::extensions`. An extension that is off is
//! refused by the server (see `routes::extensions::extension_fence`), not
//! merely hidden, and turning one off never deletes its records. Every change
//! is written to the loca's Journal with who made it.

use protocol::{extension_spec, SenderType, ServerFrame, EXTENSIONS};

use super::Hub;
use crate::sync::RecoverMutex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionError {
    /// A name that is not in the catalog.
    Unknown(String),
    /// A direct message is conversation; it has no extensions.
    Dm,
    /// The loca is closed (read-only) or sealed.
    NotWritable,
    /// The Loca Board turned on in a loca with no Lead.
    BoardNeedsLead,
    Storage,
}

impl ExtensionError {
    pub fn message(&self) -> String {
        match self {
            Self::Unknown(name) => format!("unknown extension: {name}"),
            Self::Dm => "a birebir has no extensions".into(),
            Self::NotWritable => "this loca is closed — read-only".into(),
            Self::BoardNeedsLead => "the Loca Board needs a Lead: select a Lead first".into(),
            Self::Storage => "could not save extensions — try again".into(),
        }
    }
}

/// What one change did, for the response and the Journal line.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ExtensionChange {
    pub enabled: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// How a caller asks for a loca's enabled set to change.
#[derive(Debug, Clone, Copy)]
pub enum ExtensionEdit<'a> {
    /// The whole set, as the create and manage screens send it.
    Replace(&'a [String]),
    /// One extension on, the rest as they are.
    Add(&'a str),
    /// One extension off, the rest as they are.
    Remove(&'a str),
}

/// Check a requested set against the catalog and put it in catalog order,
/// without duplicates. Chat is not an extension and is rejected like any
/// other unknown name.
pub fn normalize_extensions(wanted: &[String]) -> Result<Vec<String>, ExtensionError> {
    for name in wanted {
        if extension_spec(name.trim()).is_none() {
            return Err(ExtensionError::Unknown(name.clone()));
        }
    }
    Ok(EXTENSIONS
        .iter()
        .filter(|spec| wanted.iter().any(|name| name.trim() == spec.id))
        .map(|spec| spec.id.to_string())
        .collect())
}

fn display_list(ids: &[String]) -> String {
    ids.iter()
        .map(|id| {
            extension_spec(id)
                .map(|spec| spec.name)
                .unwrap_or(id.as_str())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The Journal line for a change, e.g. `extensions: added Focus; removed Live`.
pub fn change_line(added: &[String], removed: &[String]) -> String {
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("added {}", display_list(added)));
    }
    if !removed.is_empty() {
        parts.push(format!("removed {}", display_list(removed)));
    }
    format!("extensions: {}", parts.join("; "))
}

/// The Journal line written when a loca is opened with a chosen set.
pub fn opened_line(enabled: &[String]) -> String {
    if enabled.is_empty() {
        "extensions: opened with chat only".into()
    } else {
        format!("extensions: opened with {}", display_list(enabled))
    }
}

impl Hub {
    /// Whether extension `id` is on in `room`. A direct message has none, whatever
    /// its stored settings say.
    pub fn extension_enabled(&self, room: &str, id: &str) -> bool {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return false;
        }
        self.settings(room).extension_enabled(id)
    }

    /// The enabled extension ids of `room`, in catalog order.
    pub fn enabled_extensions(&self, room: &str) -> Vec<String> {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return Vec::new();
        }
        self.settings(room).enabled_extensions()
    }

    /// Replace the enabled set. The caller has already checked authority
    /// (operator, Master or Smaster). Turning `live` off also ends live mode,
    /// so a loca cannot stay live with no toggle to stop it. Records are never
    /// deleted. A real change is written to the Journal by `by`.
    #[cfg(test)]
    pub fn set_extensions(
        &self,
        room: &str,
        wanted: &[String],
        by: &str,
        by_type: SenderType,
    ) -> Result<ExtensionChange, ExtensionError> {
        self.edit_extensions(room, ExtensionEdit::Replace(wanted), by, by_type)
    }

    /// Change the enabled set: replace it, or add or remove one extension.
    /// A single add or remove is worked out from the set as it stands under
    /// the lock, never from a copy the caller read earlier, so two people
    /// changing different extensions at once both keep their change.
    #[cfg(test)]
    pub fn edit_extensions(
        &self,
        room: &str,
        edit: ExtensionEdit<'_>,
        by: &str,
        by_type: SenderType,
    ) -> Result<ExtensionChange, ExtensionError> {
        self.edit_extensions_noting(room, edit, by, by_type, None)
    }

    /// `edit_extensions`, with `note` added to the Journal line when the
    /// change removes the Loca Board (what became of its unfinished tasks).
    pub fn edit_extensions_noting(
        &self,
        room: &str,
        edit: ExtensionEdit<'_>,
        by: &str,
        by_type: SenderType,
        note: Option<&str>,
    ) -> Result<ExtensionChange, ExtensionError> {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(ExtensionError::Dm);
        }
        // Refuse an unknown id before anything else, whatever the loca's state.
        match edit {
            ExtensionEdit::Replace(wanted) => {
                normalize_extensions(wanted)?;
            }
            ExtensionEdit::Add(id) | ExtensionEdit::Remove(id) => {
                normalize_extensions(&[id.to_string()])?;
            }
        }
        if !self.is_writable(room) {
            return Err(ExtensionError::NotWritable);
        }
        // The setting and the Journal line that announces it are one write:
        // a failed Journal line must not leave the loca changed with nobody
        // told, and the mirrors below change only after the commit.
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        let before = r.settings.enabled_extensions();
        let enabled = match edit {
            ExtensionEdit::Replace(wanted) => normalize_extensions(wanted)?,
            ExtensionEdit::Add(id) => {
                let mut wanted = before.clone();
                wanted.push(id.to_string());
                normalize_extensions(&wanted)?
            }
            ExtensionEdit::Remove(id) => {
                let wanted: Vec<String> =
                    before.iter().filter(|have| *have != id).cloned().collect();
                normalize_extensions(&wanted)?
            }
        };
        let enabled = super::work::settle_board(&before, enabled, r.settings.lead.is_some())?;
        let added: Vec<String> = enabled
            .iter()
            .filter(|id| !before.contains(id))
            .cloned()
            .collect();
        let removed: Vec<String> = before
            .iter()
            .filter(|id| !enabled.contains(id))
            .cloned()
            .collect();
        let mut s = r.settings.clone();
        s.extensions = Some(enabled.clone());
        if !s.extension_enabled("live") {
            s.live = false;
        }
        if added.iter().any(|id| id == "board") {
            s.board_since = Some((self.now_ms)());
        }
        let mut text = change_line(&added, &removed);
        if let Some(note) = note.filter(|_| removed.iter().any(|id| id == "board")) {
            text = format!("{text} ({note})");
        }
        let entry = (!added.is_empty() || !removed.is_empty()).then(|| protocol::JournalEntry {
            id: r.next_journal_id,
            room: room.to_string(),
            by: by.to_string(),
            by_type,
            text,
            at: (self.now_ms)(),
        });
        match entry.as_ref() {
            Some(entry) => self.store.save_room_with_journal(room, &r.mode, &s, entry),
            None if s != r.settings => self.store.save_room(room, &r.mode, &s),
            None => Ok(()),
        }
        .map_err(|_| ExtensionError::Storage)?;
        if s != r.settings {
            r.settings = s.clone();
            let _ = r.tx.send(ServerFrame::Settings {
                settings: self.shown_settings(s),
            });
        }
        if let Some(entry) = entry {
            r.next_journal_id = entry.id + 1;
            r.journal.push(entry.clone());
            // The line is always written ("Changes are known"). It is pushed
            // to open sockets only while the Journal is on in this loca: an
            // extension that is off is refused, not merely hidden, and that
            // includes the line that records the Journal being turned off.
            // Whoever turns the Journal on again reads it there.
            if r.settings.extension_enabled("journal") {
                let _ = r.tx.send(ServerFrame::Journal { entry });
            }
        }
        let change = ExtensionChange {
            enabled,
            added,
            removed,
        };
        Ok(change)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_orders_dedupes_and_rejects_unknown() {
        let wanted = vec!["wiki".to_string(), "notes".into(), "wiki".into()];
        assert_eq!(
            normalize_extensions(&wanted).unwrap(),
            vec!["notes".to_string(), "wiki".into()]
        );
        assert_eq!(
            normalize_extensions(&["chat".to_string()]),
            Err(ExtensionError::Unknown("chat".into()))
        );
    }

    #[test]
    fn journal_lines_name_the_extensions() {
        assert_eq!(
            change_line(&["focus".into()], &["wiki".into(), "live".into()]),
            "extensions: added Focus; removed Loca Wiki, Live"
        );
        assert_eq!(opened_line(&[]), "extensions: opened with chat only");
    }

    #[test]
    fn absent_settings_keep_every_legacy_extension() {
        let settings = protocol::RoomSettings::default();
        for id in protocol::LEGACY_EXTENSIONS {
            assert!(settings.extension_enabled(id));
        }
        let parsed: protocol::RoomSettings =
            serde_json::from_str(r#"{"rate_limit":10,"rate_window_secs":30}"#).unwrap();
        assert_eq!(parsed.extensions, None);
        assert_eq!(parsed.enabled_extensions().len(), 5);
    }
}
