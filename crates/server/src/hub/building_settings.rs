//! Building settings (`GET/PUT /building/settings`): what the Building runs
//! on, as the Master and Smasters may read it, and the one thing the Master
//! changes here at runtime, the Building's display name.
//!
//! Everything else is the deployment's: it comes from the server's
//! environment at start-up (`LOCA_CAPACITY`, `REQUIRE_INVITE`, …) and is shown
//! under the variable's own name with the value in force, never changed from
//! the browser. A setting the operator wrote in `.env` that the browser could
//! silently override would leave two places that disagree about one number.

use super::profile_card::{bidi_control, invisible};
use super::{Hub, RequestAuthority};

/// A Building name is one short line.
pub const BUILDING_NAME_MAX_CHARS: usize = 64;

#[derive(Debug, PartialEq, Eq)]
pub enum BuildingNameError {
    /// Only the Master names the Building (PRINCIPLES "Building layer": the
    /// Master owns the building and has the final word).
    MasterOnly,
    /// The name is refused; the reason is for the person typing it.
    Invalid(String),
    /// A memory-only Building (no `DB_PATH`) has nowhere to keep it.
    MemoryOnly,
    /// The write failed.
    Storage,
}

/// Validate a Building name. `Ok(None)` clears it (empty, or only spaces).
/// One line, no control, direction-control or invisible characters, runs of
/// spaces become one, at most [`BUILDING_NAME_MAX_CHARS`] characters. Display
/// only: it is never an address and nothing reads it for authority.
pub(crate) fn validate_building_name(raw: &str) -> Result<Option<String>, String> {
    let mut name = String::new();
    for c in raw.chars() {
        if c.is_control() && !c.is_whitespace() || bidi_control(c) || invisible(c) {
            return Err("a Building name holds no control or invisible characters".into());
        }
        if matches!(c, '\n' | '\r' | '\u{0085}' | '\u{2028}' | '\u{2029}') {
            return Err("a Building name is one line".into());
        }
        if c.is_whitespace() {
            if !name.is_empty() && !name.ends_with(' ') {
                name.push(' ');
            }
            continue;
        }
        name.push(c);
    }
    let name = name.trim_end().to_string();
    if name.is_empty() {
        return Ok(None);
    }
    if name.chars().count() > BUILDING_NAME_MAX_CHARS {
        return Err(format!(
            "a Building name is at most {BUILDING_NAME_MAX_CHARS} characters"
        ));
    }
    Ok(Some(name))
}

impl Hub {
    /// The Building's display name, if the Master gave it one.
    pub fn building_name(&self) -> Option<String> {
        self.store.building_name().unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the Building name");
            None
        })
    }

    /// Name the Building (or clear its name). Master only.
    pub fn set_building_name(
        &self,
        actor: &RequestAuthority,
        raw: &str,
    ) -> Result<Option<String>, BuildingNameError> {
        if !actor.is_master() {
            return Err(BuildingNameError::MasterOnly);
        }
        let name = validate_building_name(raw).map_err(BuildingNameError::Invalid)?;
        if !self.store.is_persistent() {
            return Err(BuildingNameError::MemoryOnly);
        }
        self.store
            .set_building_name(name.as_deref(), actor.principal_id.as_deref(), self.now())
            .map_err(|error| {
                tracing::warn!(%error, "could not save the Building name");
                BuildingNameError::Storage
            })?;
        Ok(name)
    }

    /// The settings view for a Building rank. `public_server` is
    /// `PUBLIC_SERVER_URL` (the route reads the environment). Values are the
    /// ones in force, after the server's own clamping.
    pub fn building_settings(
        &self,
        viewer: &RequestAuthority,
        public_server: Option<String>,
    ) -> serde_json::Value {
        let master = viewer.is_master();
        serde_json::json!({
            "name": self.building_name(),
            "name_max_chars": BUILDING_NAME_MAX_CHARS,
            // What this viewer may change here: the Master the name, a
            // Smaster nothing.
            "editable": if master { vec!["name"] } else { Vec::new() },
            "version": env!("CARGO_PKG_VERSION"),
            // DB_PATH set: kept on disk. Its path is not shown.
            "persistent": self.store.is_persistent(),
            // Read at start-up; changed only by editing the environment and
            // restarting the server.
            "environment": {
                "PUBLIC_SERVER_URL": public_server,
                "REQUIRE_INVITE": self.require_invite,
                "REQUIRE_SESSIONS": self.require_sessions,
                "LOCA_CAPACITY": self.loca_capacity(),
                "LOCA_CAPACITY_MAX": self.loca_capacity_max(),
                "RATE_LIMIT": self.default_settings.rate_limit,
                "RATE_WINDOW_SECS": self.default_settings.rate_window_secs,
                "NOTIFICATION_RATE_LIMIT": self.notification_send_limit(),
                "LOCA_CARETAKERS": self.caretaker_names(),
                "LOCA_AGENT_ROOM": self.home_room(),
            },
            // There is no download of the whole Building: a verified snapshot
            // is taken offline on the host (docs/backup-restore.md).
            "backup": {
                "download": false,
                "doc": "docs/backup-restore.md",
            },
        })
    }
}
