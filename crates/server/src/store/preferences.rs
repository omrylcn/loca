//! Personal appearance preferences, one validated JSON document per principal.
//!
//! These are cosmetic and private: only the signed-in principal reads or
//! writes their own row. Every value is checked against a closed allow-list so
//! the browser can put it straight into a `data-*` attribute.

use super::*;

pub const PREFERENCE_THEMES: &[&str] = &["dark", "light", "ember", "contrast", "paper", "night"];
/// `custom` means the colour in `accent_custom`; the browser keeps it
/// readable on the chosen theme (and falls back to teal when none is set).
pub const PREFERENCE_ACCENTS: &[&str] = &[
    "teal", "blue", "violet", "amber", "rose", "green", "orange", "sky", "indigo", "custom",
];
// Each list starts with its default. Values are only ever added: a stored
// value keeps meaning what it meant when it was saved.
pub const PREFERENCE_DENSITIES: &[&str] = &["comfortable", "compact", "spacious"];
pub const PREFERENCE_FONT_SIZES: &[&str] = &["normal", "large", "small", "larger"];
/// Typefaces already on the device (no download, no external request):
/// the monospace design face, the system UI sans and the system serif.
pub const PREFERENCE_FONT_FAMILIES: &[&str] = &["mono", "sans", "serif"];
/// Reading: message cards (others left, you right) or one flat column.
pub const PREFERENCE_LAYOUTS: &[&str] = &["bubbles", "flat"];
/// `auto` is the device's own clock style.
pub const PREFERENCE_TIME_FORMATS: &[&str] = &["auto", "h24", "h12", "relative"];
/// How wide a message may run: the line length.
pub const PREFERENCE_MEASURES: &[&str] = &["normal", "narrow", "wide"];
/// Whether one's own messages are tinted with the accent.
pub const PREFERENCE_OWN_TINTS: &[&str] = &["on", "off"];
/// `system` follows the device's reduced-motion setting; `reduced` always stills.
pub const PREFERENCE_MOTIONS: &[&str] = &["system", "reduced"];

/// `accent_custom`: empty, or a colour as `#rrggbb` (stored lower-case), so
/// the browser can put it straight into a CSS custom property.
fn custom_accent(value: &serde_json::Value) -> Option<String> {
    let text = value.as_str()?;
    if text.is_empty() {
        return Some(String::new());
    }
    let hex = text.strip_prefix('#')?;
    (hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| format!("#{}", hex.to_ascii_lowercase()))
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Preferences {
    pub theme: String,
    pub accent: String,
    pub density: String,
    pub font_size: String,
    pub font_family: String,
    pub accent_custom: String,
    pub layout: String,
    pub time_format: String,
    pub measure: String,
    pub own_tint: String,
    pub motion: String,
}

impl Default for Preferences {
    fn default() -> Self {
        Preferences {
            theme: "dark".into(),
            accent: "teal".into(),
            density: "comfortable".into(),
            font_size: "normal".into(),
            font_family: "mono".into(),
            accent_custom: String::new(),
            layout: "bubbles".into(),
            time_format: "auto".into(),
            measure: "normal".into(),
            own_tint: "on".into(),
            motion: "system".into(),
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
            if key == "accent_custom" {
                let Some(colour) = custom_accent(value) else {
                    return Err(PreferenceError::Invalid(
                        "`accent_custom` must be \"\" or a colour written #rrggbb".into(),
                    ));
                };
                next.accent_custom = colour;
                continue;
            }
            let (allowed, slot) = match key.as_str() {
                "theme" => (PREFERENCE_THEMES, &mut next.theme),
                "accent" => (PREFERENCE_ACCENTS, &mut next.accent),
                "density" => (PREFERENCE_DENSITIES, &mut next.density),
                "font_size" => (PREFERENCE_FONT_SIZES, &mut next.font_size),
                "font_family" => (PREFERENCE_FONT_FAMILIES, &mut next.font_family),
                "layout" => (PREFERENCE_LAYOUTS, &mut next.layout),
                "time_format" => (PREFERENCE_TIME_FORMATS, &mut next.time_format),
                "measure" => (PREFERENCE_MEASURES, &mut next.measure),
                "own_tint" => (PREFERENCE_OWN_TINTS, &mut next.own_tint),
                "motion" => (PREFERENCE_MOTIONS, &mut next.motion),
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
