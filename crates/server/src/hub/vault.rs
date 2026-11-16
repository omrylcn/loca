//! The Vault's rules document (PRINCIPLES "Vault"), the first step of the
//! `vault` extension: one plain text per loca, written only by those who run
//! the loca (the caller checks that, with the same check as the loca's
//! extensions) and read by everyone seated there, agents included. The Vault
//! informs; it grants nothing, and the server never reads it for meaning.
//! Every write is a new revision with who and when, the older ones are kept,
//! and a Journal line records it in the same write.

use protocol::{SenderType, ServerFrame, VaultDoc};

use super::Hub;
use crate::store::VaultRevision;
use crate::sync::RecoverMutex;

/// The most a rules document may hold, in bytes of UTF-8: a page of rules,
/// small enough to put in one model turn.
pub const VAULT_MAX_BYTES: usize = 8 * 1024;
/// How many revisions `GET /rooms/:id/vault/history` answers, newest first.
pub const VAULT_HISTORY_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultError {
    /// Over the cap, or a character that hides itself or breaks a line other
    /// than a plain line break; the sentence says which.
    Invalid(String),
    TooLong,
    /// Written against a revision that is no longer the newest.
    Stale {
        current: u64,
    },
    /// A direct message has no extensions, so no Vault.
    Dm,
    /// The loca is closed (read-only) or sealed.
    NotWritable,
    /// This deployment keeps nothing on disk; the Vault keeps every revision.
    NoStorage,
    Storage,
}

impl VaultError {
    pub fn message(&self) -> String {
        match self {
            Self::Invalid(text) => text.clone(),
            Self::TooLong => {
                format!("the rules are too long: at most {VAULT_MAX_BYTES} bytes")
            }
            Self::Stale { current } => format!(
                "the rules changed since you read them: the newest revision is {current}. \
                 Read them again and write against that revision."
            ),
            Self::Dm => "a birebir has no Vault".into(),
            Self::NotWritable => "this loca is closed — read-only".into(),
            Self::NoStorage => "the Vault needs persistent storage in this deployment".into(),
            Self::Storage => "could not save the rules — try again".into(),
        }
    }
}

/// Check a rules document as it is stored: within the cap, and no control or
/// invisible formatting character other than a line break. Nothing else is
/// read: the text is never interpreted.
pub fn validated_body(body: &str) -> Result<(), VaultError> {
    if body.len() > VAULT_MAX_BYTES {
        return Err(VaultError::TooLong);
    }
    if body
        .chars()
        .any(|c| c != '\n' && super::identity::hidden_or_breaking(c))
    {
        return Err(VaultError::Invalid(
            "the rules must be plain text: line breaks are allowed, but no other control or \
             invisible formatting characters"
                .into(),
        ));
    }
    Ok(())
}

impl Hub {
    /// The newest rules of `room`; revision 0 with an empty text before the
    /// first write.
    pub fn vault(&self, room: &str) -> rusqlite::Result<VaultDoc> {
        Ok(self.store.vault(room)?.unwrap_or_else(|| VaultDoc {
            room: room.to_string(),
            rev: 0,
            body: String::new(),
            updated_by: None,
            updated_at: None,
        }))
    }

    /// Up to [`VAULT_HISTORY_LIMIT`] revisions of `room`'s rules, newest first.
    pub fn vault_history(&self, room: &str) -> rusqlite::Result<Vec<VaultDoc>> {
        self.store.vault_history(room, VAULT_HISTORY_LIMIT)
    }

    /// Write a new revision of `room`'s rules against `base_rev`, the
    /// revision the writer read. The caller has already checked authority.
    /// The revision, the settings that name it (`vault_rev`) and the Journal
    /// line are one write; then everyone seated hears a `vault` frame.
    pub fn write_vault(
        &self,
        room: &str,
        body: &str,
        base_rev: u64,
        by: &str,
        by_type: SenderType,
        by_principal: Option<&str>,
    ) -> Result<VaultDoc, VaultError> {
        if self.is_dm(room) || room.starts_with(Self::DM_PREFIX) {
            return Err(VaultError::Dm);
        }
        validated_body(body)?;
        if !self.store.is_persistent() {
            return Err(VaultError::NoStorage);
        }
        if !self.is_writable(room) {
            return Err(VaultError::NotWritable);
        }
        let numbers = self.seat_numbers();
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        let current = r.settings.vault_rev.unwrap_or(0);
        if base_rev != current {
            return Err(VaultError::Stale { current });
        }
        let rev = current + 1;
        let at = (self.now_ms)();
        let mut settings = r.settings.clone();
        settings.vault_rev = Some(rev);
        let entry = protocol::JournalEntry {
            id: r.next_journal_id,
            room: room.to_string(),
            by: by.to_string(),
            by_type,
            text: if body.is_empty() {
                format!("vault: rules cleared (revision {rev})")
            } else {
                format!("vault: rules updated (revision {rev})")
            },
            at,
        };
        self.store
            .save_vault_revision(
                &VaultRevision {
                    room,
                    rev,
                    body,
                    by,
                    by_type,
                    by_principal,
                    at,
                },
                &r.mode,
                &settings,
                &entry,
            )
            .map_err(|_| VaultError::Storage)?;
        r.settings = settings.clone();
        r.next_journal_id = entry.id + 1;
        r.journal.push(entry.clone());
        let _ = r.tx.send(ServerFrame::Settings {
            settings: super::identity::shown(settings, numbers),
        });
        let _ = r.tx.send(ServerFrame::Vault {
            room: room.to_string(),
            rev,
            updated_by: by.to_string(),
            updated_at: at,
        });
        // Written always ("Changes are known"); pushed only while the
        // Journal is on, like an extension change.
        if r.settings.extension_enabled("journal") {
            let _ = r.tx.send(ServerFrame::Journal { entry });
        }
        Ok(VaultDoc {
            room: room.to_string(),
            rev,
            body: body.to_string(),
            updated_by: Some(by.to_string()),
            updated_at: Some(at),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rules_are_plain_text_within_the_cap() {
        assert_eq!(validated_body(""), Ok(()));
        assert_eq!(
            validated_body(
                "# Rules\n\n- run `make check` before saying done\n- ask before deploy/"
            ),
            Ok(())
        );
        // Exactly the cap, counted in bytes: two-byte letters fill it at half
        // the characters.
        assert_eq!(validated_body(&"a".repeat(VAULT_MAX_BYTES)), Ok(()));
        assert_eq!(validated_body(&"é".repeat(VAULT_MAX_BYTES / 2)), Ok(()));
        assert_eq!(
            validated_body(&"a".repeat(VAULT_MAX_BYTES + 1)),
            Err(VaultError::TooLong)
        );
        assert_eq!(
            validated_body(&"é".repeat(VAULT_MAX_BYTES / 2 + 1)),
            Err(VaultError::TooLong)
        );
        for bad in [
            "carriage\r\nreturn",
            "a\ttab",
            "line\u{2028}separator",
            "zero\u{200B}width",
            "right\u{202E}to left",
            "bom\u{FEFF}",
            "tag\u{E0041}",
            "bell\u{7}",
        ] {
            let error = validated_body(bad).unwrap_err();
            assert!(error.message().contains("plain text"), "{bad:?}");
        }
    }
}
