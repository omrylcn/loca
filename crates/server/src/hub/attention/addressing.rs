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
        // A reply addresses its author as a separate recipient, so a reply to a
        // caretaker summons them just like an explicit target/@mention. The
        // sort+dedup below collapses the duplicate when the same caretaker is
        // also targeted or mentioned, keeping exactly one summon.
        if let Some(author) = message.reply_to_sender.as_deref() {
            let author = author.to_lowercase();
            if self.caretakers.contains(&author) {
                addressed.push(author);
            }
        }
        addressed.extend(
            crate::hub::mentioned_names(&message.text)
                .into_iter()
                .filter(|name| self.caretakers.contains(name)),
        );
        addressed.sort();
        addressed.dedup();
        addressed
    }
}
