//! Wait and Reminder are core: the reminder sweep runs in every loca but a
//! direct message, whatever its extensions are. A declared wait, a wait
//! cycle and an enabled silence check belong to no extension. A reminder
//! about a goal or a task does: it is raised only while the extension that
//! serves those records is on, so it never wakes anybody into a 403 for
//! something the loca does not show.

use protocol::RoomSettings;

/// Whether reminders about the records under `/rooms/:id/<segment>` may be
/// raised in a loca with these settings: no extension owns them, or the one
/// that does (`protocol::EXTENSIONS`) is on.
pub(super) fn serves(settings: &RoomSettings, segment: &str) -> bool {
    protocol::extension_of_segment(segment).is_none_or(|spec| settings.extension_enabled(spec.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_follow_the_extension_that_serves_them() {
        let mut settings = RoomSettings {
            extensions: Some(vec!["notes".into()]),
            ..RoomSettings::default()
        };
        assert!(!serves(&settings, "tasks"));
        assert!(!serves(&settings, "goals"));
        assert!(serves(&settings, "waits"), "no extension owns waits");
        settings.extensions = Some(vec!["focus".into()]);
        assert!(!serves(&settings, "tasks"), "tasks are the Loca Board's");
        assert!(serves(&settings, "goals"));
        settings.extensions = Some(vec!["board".into()]);
        assert!(serves(&settings, "tasks"));
        assert!(!serves(&settings, "goals"));
    }
}
