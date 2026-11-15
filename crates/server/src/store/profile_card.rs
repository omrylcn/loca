//! A person's optional full name and profile icon, kept on the principal row
//! so they follow the person to every loca and session.
//!
//! Display only: neither is unique, neither is an address, and nothing reads
//! them to decide authority, routing, deduplication or attribution. The hub
//! validates both (`hub/profile_card.rs`) before anything is written here.

use std::collections::HashMap;

use super::*;

/// What a person chose to be shown as, beside the `@name` they are.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ProfileCard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub full_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl ProfileCard {
    pub fn is_empty(&self) -> bool {
        self.full_name.is_none() && self.icon.is_none()
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
    }
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

fn card_of(full_name: Option<String>, icon: Option<String>) -> ProfileCard {
    ProfileCard {
        full_name: full_name.filter(|value| !value.is_empty()),
        icon: icon.filter(|value| !value.is_empty()),
    }
}

impl Store {
    /// The principal's card; empty when none is set or nothing is stored.
    pub fn profile_card(&self, principal_id: &str) -> ProfileCard {
        let Some(c) = self.conn() else {
            return ProfileCard::default();
        };
        c.query_row(
            "SELECT full_name, icon FROM principals WHERE id = ?1 AND disabled_at IS NULL",
            params![principal_id],
            |row| Ok(card_of(row.get(0)?, row.get(1)?)),
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
        let Ok(mut statement) = c.prepare(
            "SELECT id, full_name, icon FROM principals
              WHERE disabled_at IS NULL AND (full_name IS NOT NULL OR icon IS NOT NULL)",
        ) else {
            return HashMap::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, card_of(row.get(1)?, row.get(2)?)))
        }) else {
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
        let Ok(mut statement) = c.prepare(
            "SELECT display_name, id, full_name, icon,
                    (SELECT COUNT(*) FROM principals other
                      WHERE other.display_name = principals.display_name
                        AND other.disabled_at IS NULL)
               FROM principals
              WHERE disabled_at IS NULL",
        ) else {
            return HashMap::new();
        };
        let Ok(rows) = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                card_of(row.get(2)?, row.get(3)?),
                row.get::<_, i64>(4)?,
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
        full_name: Option<Option<&str>>,
        icon: Option<Option<&str>>,
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
            if let Some(value) = full_name {
                tx.execute(
                    "UPDATE principals SET full_name = ?2 WHERE id = ?1",
                    params![principal_id, value],
                )?;
            }
            if let Some(value) = icon {
                tx.execute(
                    "UPDATE principals SET icon = ?2 WHERE id = ?1",
                    params![principal_id, value],
                )?;
            }
            let card = tx.query_row(
                "SELECT full_name, icon FROM principals WHERE id = ?1",
                params![principal_id],
                |row| Ok(card_of(row.get(0)?, row.get(1)?)),
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
