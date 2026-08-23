//! Where the console remembers your buildings.
//!
//! There can be more than one. They have different master keys, and sending
//! one building's key to another is a credential disclosure — so each
//! building is stored whole, and switching means switching both address and
//! key together, never one of them. First run is deliberately blank: the
//! operator must choose the origin before entering a key.
//!
//! `~/.loca/admin.toml`, mode 600. Hand-editable; a tiny parser rather than a
//! TOML crate, because the shape is three fields and a list.

use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Default)]
pub struct Building {
    pub label: String,
    pub server: String,
    pub admin_token: String,
}

#[derive(Default)]
pub struct Settings {
    pub buildings: Vec<Building>,
    /// Index into `buildings` — the one the console is talking to.
    pub current: usize,
}

pub fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

impl Settings {
    pub fn path() -> PathBuf {
        home().join(".loca").join("admin.toml")
    }

    pub fn load() -> Self {
        let Ok(text) = fs::read_to_string(Self::path()) else {
            return Settings::load_defaults();
        };

        let mut out = Settings::default();
        let mut cur: Option<Building> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(label) = line.strip_prefix("[[").and_then(|l| l.strip_suffix("]]")) {
                if let Some(b) = cur.take() {
                    out.buildings.push(b);
                }
                cur = Some(Building {
                    label: label.trim().to_string(),
                    ..Default::default()
                });
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim().trim_matches('"').to_string());
            match (k, cur.as_mut()) {
                ("current", _) => out.current = v.parse().unwrap_or(0),
                ("server", Some(b)) => b.server = v,
                ("admin_token", Some(b)) => b.admin_token = v,
                _ => {}
            }
        }
        if let Some(b) = cur.take() {
            out.buildings.push(b);
        }
        if out.buildings.is_empty() {
            return Settings::load_defaults();
        }
        out.current = out.current.min(out.buildings.len() - 1);
        out
    }

    fn load_defaults() -> Self {
        Settings {
            buildings: vec![Building {
                label: "building".into(),
                server: String::new(),
                admin_token: String::new(),
            }],
            current: 0,
        }
    }

    pub fn save(&self) -> io::Result<PathBuf> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut text = String::from("# loca master console — buildings\n");
        text.push_str(&format!("current = {}\n\n", self.current));
        for b in &self.buildings {
            text.push_str(&format!("[[{}]]\n", b.label));
            text.push_str(&format!("server = \"{}\"\n", b.server));
            text.push_str(&format!("admin_token = \"{}\"\n\n", b.admin_token));
        }
        fs::write(&path, text)?;
        // The master key is in here; nobody else on the machine reads it.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(path)
    }

    pub fn current(&self) -> Building {
        self.buildings
            .get(self.current)
            .cloned()
            .unwrap_or_default()
    }

    pub fn current_mut(&mut self) -> Option<&mut Building> {
        self.buildings.get_mut(self.current)
    }

    /// Move to the next building — the console's whole point is that this is
    /// one keypress, not a trip through a settings screen.
    pub fn next_building(&mut self) {
        if !self.buildings.is_empty() {
            self.current = (self.current + 1) % self.buildings.len();
        }
    }

    pub fn is_configured(&self) -> bool {
        let b = self.current();
        !b.server.is_empty() && !b.admin_token.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_never_selects_a_remote_origin() {
        let settings = Settings::load_defaults();
        let building = settings.current();

        assert_eq!(building.label, "building");
        assert!(building.server.is_empty());
        assert!(building.admin_token.is_empty());
        assert!(!settings.is_configured());
    }
}
