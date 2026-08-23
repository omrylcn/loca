//! Single-owner selection for manual Attention and automated Care delivery.

use std::collections::HashMap;

use protocol::{AttentionAudience, CareSignal, ReminderRecipient};

use super::super::{Hub, Room};
use crate::sync::RecoverMutex;

impl Hub {
    pub(in crate::hub) fn care_owner(
        &self,
        rooms: &HashMap<String, Room>,
        room_name: &str,
    ) -> Option<String> {
        let room = rooms.get(room_name)?;
        // Transport presence alone is not enough: a dead wake bridge used to
        // keep the recipient green and swallow care for hours. The operator
        // may address Reminders to the dynamic room lead or one exact person;
        // either is used only while its independently supervised adapter is
        // healthy. Loca-care remains the single availability fallback.
        let mut candidates = match &room.settings.care_recipient {
            ReminderRecipient::Lead => room.settings.lead.clone().into_iter().collect(),
            ReminderRecipient::Person { name } => vec![name.clone()],
            ReminderRecipient::All => {
                let mut names: Vec<String> = room
                    .members
                    .values()
                    .filter(|(_, _, count)| *count > 0)
                    .map(|(name, _, _)| name.clone())
                    .collect();
                names.sort();
                names.dedup();
                if let Some(lead) = room.settings.lead.as_ref() {
                    names.retain(|name| name != lead);
                    names.insert(0, lead.clone());
                }
                names
            }
        };
        for recipient in candidates.drain(..) {
            if room
                .members
                .values()
                .any(|(name, _, count)| name == &recipient && *count > 0)
                && self
                    .runtime_health_for(&recipient)
                    .is_some_and(|runtime| runtime.ready)
            {
                return Some(recipient);
            }
        }
        if !self.caretakers.contains("loca-care") {
            return None;
        }
        rooms.get(self.home_room.as_str()).and_then(|home| {
            home.members
                .values()
                .any(|(name, _, count)| name == "loca-care" && *count > 0)
                .then(|| self.runtime_health_for("loca-care"))
                .flatten()
                .filter(|runtime| runtime.ready)
                .map(|_| "loca-care".to_string())
        })
    }
    /// Re-home a caretaker relay so the delivered envelope's room matches the
    /// caretaker's home loca (the socket it is delivered on), preserving the
    /// true origin in `source_room`. Only the `loca-care` fallback is relayed
    /// home; every other owner is delivered in its own room, so its envelope is
    /// left untouched and `source_room` stays empty (origin == delivery room).
    pub(in crate::hub) fn rehome_caretaker_signal(
        &self,
        signal: &mut CareSignal,
        source_room: &str,
    ) {
        if signal.owner.as_deref() == Some("loca-care") && source_room != self.home_room.as_str() {
            signal.source_room = source_room.to_string();
            signal.room = self.home_room.as_ref().clone();
        }
    }
    pub fn caretaker_owns_attention(&self, room: &str, id: &str, actor: &str) -> bool {
        self.caretakers.contains(actor)
            && self
                .rooms
                .lock_or_recover()
                .get(room)
                .and_then(|room| room.attentions.get(id))
                .is_some_and(|attention| attention.owner.as_deref() == Some(actor))
    }
    pub(in crate::hub) fn resolve_attention_owner(
        &self,
        room: &Room,
        audience: &AttentionAudience,
    ) -> Option<(String, Vec<String>)> {
        match audience {
            AttentionAudience::Lead => room
                .settings
                .lead
                .clone()
                .map(|lead| (lead.clone(), vec![lead])),
            AttentionAudience::Person { name } => {
                let name = name.trim().to_string();
                (!name.is_empty()).then(|| (name.clone(), vec![name]))
            }
            AttentionAudience::Group { names } => {
                let mut names: Vec<String> = names
                    .iter()
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect();
                names.sort();
                names.dedup();
                let owner = names
                    .iter()
                    .find(|name| {
                        let seated = room
                            .members
                            .values()
                            .any(|(member, _, connections)| member == *name && *connections > 0);
                        seated
                            && self
                                .runtime_health_for(name)
                                .is_some_and(|runtime| runtime.ready)
                    })
                    .or_else(|| names.first())?
                    .clone();
                Some((owner, names))
            }
        }
    }
}
