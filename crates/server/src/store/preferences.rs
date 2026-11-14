//! Personal appearance preferences, one validated JSON document per principal.
//!
//! These are cosmetic and private: only the signed-in principal reads or
//! writes their own row. Every value is checked against a closed allow-list so
//! the browser can put it straight into a `data-*` attribute.

use super::*;

pub const PREFERENCE_THEMES: &[&str] = &["dark", "light"];
pub const PREFERENCE_ACCENTS: &[&str] = &["teal", "blue", "violet", "amber", "rose"];
pub const PREFERENCE_DENSITIES: &[&str] = &["comfortable", "compact"];
pub const PREFERENCE_FONT_SIZES: &[&str] = &["normal", "large"];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Preferences {
    pub theme: String,
    pub accent: String,
    pub density: String,
    pub font_size: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            theme: "dark".into(),
            accent: "teal".into(),
            density: "comfortable".into(),
            font_size: "normal".into(),
        }
    }
}

#[derive(Debug)]
pub enum PreferenceError {
    /// The patch named an unknown field or a value outside the allowed set.
    Invalid(String),
    /// No persistent store, or the write failed.
    Storage,
}

impl Preferences {
    /// Apply a partial update. Fields left out keep their current value;
    /// any unknown field or disallowed value rejects the whole patch.
    pub fn merged(&self, patch: &serde_json::Value) -> Result<Preferences, PreferenceError> {
        let Some(fields) = patch.as_object() else {
            return Err(PreferenceError::Invalid(
                "preferences must be a JSON object".into(),
            ));
        };
        let mut next = self.clone();
        for (key, value) in fields {
            let (allowed, slot) = match key.as_str() {
                "theme" => (PREFERENCE_THEMES, &mut next.theme),
                "accent" => (PREFERENCE_ACCENTS, &mut next.accent),
                "density" => (PREFERENCE_DENSITIES, &mut next.density),
                "font_size" => (PREFERENCE_FONT_SIZES, &mut next.font_size),
                _ => {
                    return Err(PreferenceError::Invalid(format!(
                        "unknown preference `{key}`"
                    )))
                }
            };
            match value.as_str() {
                Some(text) if allowed.contains(&text) => *slot = text.to_string(),
                _ => {
                    return Err(PreferenceError::Invalid(format!(
                        "`{key}` must be one of: {}",
                        allowed.join(", ")
                    )))
                }
            }
        }
        Ok(next)
    }

    /// Read a stored row leniently: a value retired from the allow-list falls
    /// back to its default instead of breaking the whole document.
    fn from_stored(json: &str) -> Preferences {
        let mut prefs = Preferences::default();
        if let Ok(serde_json::Value::Object(fields)) = serde_json::from_str(json) {
            for (key, value) in fields {
                let single = serde_json::json!({ key: value });
                if let Ok(next) = prefs.merged(&single) {
                    prefs = next;
                }
            }
        }
        prefs
    }
}

impl Store {
    fn read_preferences(c: &Connection, principal_id: &str) -> Preferences {
        c.query_row(
            "SELECT json FROM principal_preferences WHERE principal_id = ?1",
            params![principal_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|json| Preferences::from_stored(&json))
        .unwrap_or_default()
    }

    /// The principal's preferences, or the defaults when none are saved.
    pub fn principal_preferences(&self, principal_id: &str) -> Preferences {
        match self.conn() {
            Some(c) => Self::read_preferences(&c, principal_id),
            None => Preferences::default(),
        }
    }

    /// Validate `patch` against the current row and save the result, under
    /// one held connection so concurrent partial updates cannot interleave.
    pub fn merge_principal_preferences(
        &self,
        principal_id: &str,
        patch: &serde_json::Value,
        at: u64,
    ) -> Result<Preferences, PreferenceError> {
        let Some(c) = self.conn() else {
            // Validate anyway so a memory-only server still answers 400 first.
            Preferences::default().merged(patch)?;
            return Err(PreferenceError::Storage);
        };
        let next = Self::read_preferences(&c, principal_id).merged(patch)?;
        let json = serde_json::to_string(&next).map_err(|_| PreferenceError::Storage)?;
        c.execute(
            "INSERT INTO principal_preferences (principal_id, json, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(principal_id) DO UPDATE SET json = excluded.json,
                                                     updated_at = excluded.updated_at",
            params![principal_id, json, at],
        )
        .map_err(|_| PreferenceError::Storage)?;
        Ok(next)
    }
}
