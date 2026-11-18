//! Sounds: which arrivals the web client chimes for, and how loudly.
//!
//! Like the look, these are one person's own and change nothing for anyone
//! else. The keys sit flat in the same preferences document
//! (`GET/PUT /profile/preferences`) so an older client that sends only the
//! look keeps them, and every value is from a closed list.

/// A switch that starts on.
pub const PREFERENCE_SOUND_ON_FIRST: &[&str] = &["on", "off"];
/// A switch that starts off: plain messages, so a busy loca is not noisy.
pub const PREFERENCE_SOUND_OFF_FIRST: &[&str] = &["off", "on"];
/// The browser maps each to a fixed loudness; no free number is stored.
pub const PREFERENCE_SOUND_VOLUMES: &[&str] = &["medium", "low", "high"];

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct SoundPreferences {
    /// The master switch.
    pub sound: String,
    /// A new message in a loca one is in.
    pub sound_message: String,
    /// A message that mentions one, replies to one or is addressed to one;
    /// a reminder addressed to one.
    pub sound_mention: String,
    /// A new private message.
    pub sound_dm: String,
    /// A Building notification (the bell).
    pub sound_notice: String,
    pub sound_volume: String,
    /// `on`: only while Loca is not the window in front.
    pub sound_background: String,
}

impl Default for SoundPreferences {
    fn default() -> Self {
        SoundPreferences {
            sound: PREFERENCE_SOUND_ON_FIRST[0].into(),
            sound_message: PREFERENCE_SOUND_OFF_FIRST[0].into(),
            sound_mention: PREFERENCE_SOUND_ON_FIRST[0].into(),
            sound_dm: PREFERENCE_SOUND_ON_FIRST[0].into(),
            sound_notice: PREFERENCE_SOUND_ON_FIRST[0].into(),
            sound_volume: PREFERENCE_SOUND_VOLUMES[0].into(),
            sound_background: PREFERENCE_SOUND_ON_FIRST[0].into(),
        }
    }
}

impl SoundPreferences {
    /// The allowed values of a sound key and where its value is kept, or
    /// `None` when `key` is not a sound key.
    pub fn slot(&mut self, key: &str) -> Option<(&'static [&'static str], &mut String)> {
        Some(match key {
            "sound" => (PREFERENCE_SOUND_ON_FIRST, &mut self.sound),
            "sound_message" => (PREFERENCE_SOUND_OFF_FIRST, &mut self.sound_message),
            "sound_mention" => (PREFERENCE_SOUND_ON_FIRST, &mut self.sound_mention),
            "sound_dm" => (PREFERENCE_SOUND_ON_FIRST, &mut self.sound_dm),
            "sound_notice" => (PREFERENCE_SOUND_ON_FIRST, &mut self.sound_notice),
            "sound_volume" => (PREFERENCE_SOUND_VOLUMES, &mut self.sound_volume),
            "sound_background" => (PREFERENCE_SOUND_ON_FIRST, &mut self.sound_background),
            _ => return None,
        })
    }
}
