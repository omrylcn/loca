//! Caretaker addressing parsed from explicit message targets and mentions.

use protocol::Message;

use super::super::Hub;

impl Hub {
    pub(in crate::hub) fn addressed_caretakers(&self, message: &Message) -> Vec<String> {
        let mut addressed = Vec::new();
        if let Some(target) = message.target.as_deref() {
            let target = target.to_lowercase();
            if self.caretakers.contains(&target) {
                addressed.push(target);
            }
        }
        addressed.extend(
            message
                .text
                .to_lowercase()
                .split(|c: char| !(c.is_alphanumeric() || c == '@' || c == '-' || c == '_'))
                .filter_map(|token| {
                    token
                        .strip_prefix('@')
                        .filter(|name| self.caretakers.contains(*name))
                        .map(str::to_string)
                }),
        );
        addressed.sort();
        addressed.dedup();
        addressed
    }
}
