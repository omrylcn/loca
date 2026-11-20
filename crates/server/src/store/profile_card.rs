//! A person's optional full name and profile icon, kept on the principal row
//! so they follow the person to every loca and session.
//!
//! Display only: neither is unique, neither is an address, and nothing reads
//! them to decide authority, routing, deduplication or attribution. The hub
//! validates both (`hub/profile_card.rs`) before anything is written here.

use std::collections::HashMap;

use super::{params, OptionalExtension, Store};

/// What a person chose to be shown as, beside the `@name` they are.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The avatar's background colour and shape, each from a closed list
    /// (`hub/profile_card.rs`). Display only, like the icon.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_shape: Option<String>,
}

impl ProfileCard {
    pub fn is_empty(&self) -> bool {
        self.full_name.is_none()
            && self.icon.is_none()
            && self.avatar_color.is_none()
            && self.avatar_shape.is_none()
    }

    /// Add `full_name` / `icon` to a JSON object describing this person,
    /// only when set: with neither, the object is exactly what it was.
    pub fn decorate(&self, object: &mut serde_json::Value) {
        let Some(fields) = object.as_object_mut() else {
            return;
        };
        if let Some(full_name) = &self.full_name {
            fields.insert("full_name".into(), full_name.clone().into());
        }
        if let Some(icon) = &self.icon {
            fields.insert("icon".into(), icon.clone().into());
        }
        if let Some(color) = &self.avatar_color {
            fields.insert("avatar_color".into(), color.clone().into());
        }
        if let Some(shape) = &self.avatar_shape {
            fields.insert("avatar_shape".into(), shape.clone().into());
        }
    }
}

/// The card fields one write sets: `None` leaves a field as it is,
/// `Some(None)` clears it, `Some(Some(value))` sets an already validated value.
#[derive(Debug, Default)]
pub struct CardPatch<'a> {
    pub full_name: Option<Option<&'a str>>,
    pub icon: Option<Option<&'a str>>,
    pub avatar_color: Option<Option<&'a str>>,
    pub avatar_shape: Option<Option<&'a str>>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProfileCardError {
    /// The body is not an object of `full_name` / `icon`, or a value is refused.
    Invalid(String),
    /// No active principal answers to the caller (revoked between the
    /// authority check and the write).
    NoProfile,
    /// No persistent store, or the write failed.
    Storage,
}

/// The card columns, in the order every query here selects them.
const CARD_COLUMNS: &str = "full_name, icon, avatar_color, avatar_shape";

fn card_at(row: &rusqlite::Row<'_>, first: usize) -> rusqlite::Result<ProfileCard> {
    let text = |at: usize| -> rusqlite::Result<Option<String>> {
        Ok(row
            .get::<_, Option<String>>(first + at)?
            .filter(|value| !value.is_empty()))
    };
    Ok(ProfileCard {
        full_name: text(0)?,
        icon: text(1)?,
        avatar_color: text(2)?,
        avatar_shape: text(3)?,
    })
}

impl Store {
    /// The principal's card; empty when none is set or nothing is stored.
    pub fn profile_card(&self, principal_id: &str) -> ProfileCard {
        let Some(c) = self.conn() else {
            return ProfileCard::default();
        };
        c.query_row(
            &format!("SELECT {CARD_COLUMNS} FROM principals WHERE id = ?1 AND disabled_at IS NULL"),
            params![principal_id],
            |row| card_at(row, 0),
        )
        .optional()
        .ok()
        .flatten()
        .unwrap_or_default()
    }

    /// Every active person's card, keyed by principal id. Only people with
    /// a card are listed.
    pub fn profile_cards_by_principal(&self) -> HashMap<String, ProfileCard> {
        let Some(c) = self.conn() else {
            return HashMap::new();
        };
        let Ok(mut statement) = c.prepare(&format!(
            "SELECT id, {CARD_COLUMNS} FROM principals
              WHERE disabled_at IS NULL
                AND (full_name IS NOT NULL OR icon IS NOT NULL
                     OR avatar_color IS NOT NULL OR avatar_shape IS NOT NULL)"
        )) else {
            return HashMap::new();
        };
        let Ok(rows) =
            statement.query_map([], |row| Ok((row.get::<_, String>(0)?, card_at(row, 1)?)))
        else {
            return HashMap::new();
        };
        rows.flatten()
            .filter(|(_, card)| !card.is_empty())
            .collect()
    }

    /// Every active person by the `@name` they wear now: their principal id
    /// and card (empty when none is set). A name two active principals share
    /// (rows older than the one-name rule) is left out, so no row is ever
    /// given the wrong person's id or card.
    pub fn people_by_name(&self) -> HashMap<String, (String, ProfileCard)> {
        let Some(c) = self.conn() else {
            return HashMap::new();
        };
        let Ok(mut statement) = c.prepare(&format!(
            "SELECT display_name, id, {CARD_COLUMNS},
                    (SELECT COUNT(*) FROM principals other
                      WHERE other.display_name = principals.display_name
                        AND other.disabled_at IS NULL)
               FROM principals
              WHERE disabled_at IS NULL"
        )) else {
            return HashMap::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                card_at(row, 2)?,
                row.get::<_, i64>(6)?,
            ))
        }) else {
            return HashMap::new();
        };
        rows.flatten()
            .filter(|(_, _, _, wearers)| *wearers == 1)
            .map(|(name, id, card, _)| (name, (id, card)))
            .collect()
    }

    /// Write the given fields of an active principal's card (`Some(None)`
    /// clears one, `None` leaves it) and return the card as stored, in one
    /// transaction so two partial writes cannot interleave.
    pub fn set_profile_card(
        &self,
        principal_id: &str,
        patch: &CardPatch<'_>,
    ) -> Result<ProfileCard, ProfileCardError> {
        let Some(mut c) = self.conn() else {
            return Err(ProfileCardError::Storage);
        };
        let mut write = || -> rusqlite::Result<Option<ProfileCard>> {
            let tx = c.transaction()?;
            let active = tx
                .query_row(
                    "SELECT 1 FROM principals WHERE id = ?1 AND disabled_at IS NULL",
                    params![principal_id],
                    |_| Ok(()),
                )
                .optional()?;
            if active.is_none() {
                return Ok(None);
            }
            // Column names are fixed here, never taken from the body.
            for (column, value) in [
                ("full_name", patch.full_name),
                ("icon", patch.icon),
                ("avatar_color", patch.avatar_color),
                ("avatar_shape", patch.avatar_shape),
            ] {
                if let Some(value) = value {
                    tx.execute(
                        &format!("UPDATE principals SET {column} = ?2 WHERE id = ?1"),
                        params![principal_id, value],
                    )?;
                }
            }
            let card = tx.query_row(
                &format!("SELECT {CARD_COLUMNS} FROM principals WHERE id = ?1"),
                params![principal_id],
                |row| card_at(row, 0),
            )?;
            tx.commit()?;
            Ok(Some(card))
        };
        match write() {
            Ok(Some(card)) => Ok(card),
            Ok(None) => Err(ProfileCardError::NoProfile),
            Err(error) => {
                tracing::error!(%error, "profile card could not be saved");
                Err(ProfileCardError::Storage)
            }
        }
    }
}
