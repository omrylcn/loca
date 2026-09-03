//! Single-owner selection for manual Attention and automated Care delivery.

use std::collections::{HashMap, HashSet};

use protocol::{AttentionAudience, CareSignal, ReminderRecipient};

use super::super::{Hub, Room};
use crate::sync::RecoverMutex;

impl Hub {
    /// The canonical recipient set for an `Everyone` reminder: `(principal_id,
    /// display_name)` derived from the room's ACTIVE davets (invites) whose
    /// building membership is still live. This is the persistent roster — it
    /// includes members who are currently offline (they are absent from the
    /// in-memory seat map, which `leave` clears at zero connections). Rules
    /// (see the multi-recipient spec): only davets for THIS room; a davet is
    /// skipped when its building membership is gone (revoked); the set is
    /// deduped by the safe canonical `principal_id`, never by display name (two
    /// principals may share a name) nor by a secret davet/member token. The
    /// `principal_id` is a stable non-secret id used internally (e.g. in the
    /// per-member attention id); the display name is what the UI shows.
    /// `Some(roster)` only when EVERY active member resolves to a principal;
    /// `None` when any does not (a roster inconsistency). The caller must treat
    /// `None` as all-or-nothing — emit no attention for the generation and leave
    /// the care mark untouched so the scheduler retries — never a partial
    /// "Everyone" that looks complete to the operator.
    pub(in crate::hub) fn everyone_recipients(&self, room: &str) -> Option<Vec<(String, String)>> {
        // Collect (member_token, display_name) under the invite/member locks,
        // then release them BEFORE the per-member store lookup — the store has
        // its own lock and this keeps the invites→members ordering short and
        // never nested with a store or rooms lock.
        let pairs: Vec<(String, String)> = {
            let invites = self.invites.lock_or_recover();
            let members = self.members.lock_or_recover();
            invites
                .values()
                .filter(|invite| invite.room == room && !invite.member.is_empty())
                .filter_map(|invite| {
                    members
                        .get(&invite.member)
                        .map(|membership| (invite.member.clone(), membership.name.clone()))
                })
                .collect()
        };
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (member_token, display_name) in pairs {
            let Some(principal_id) = self.store.principal_id_for_member_record(&member_token)
            else {
                // An active davet backed by a live membership must resolve to a
                // principal (add_member mints one at admission). If it does not,
                // the roster is inconsistent: abort the whole generation rather
                // than shrink "Everyone" to a partial audience that looks
                // complete, and never fabricate a name/token identity. The secret
                // member token is never logged — only the public display name.
                tracing::error!(
                    room = %room,
                    member = %display_name,
                    "Everyone roster inconsistency: an active member has no resolvable principal; aborting this reminder generation"
                );
                return None;
            };
            if seen.insert(principal_id.clone()) {
                out.push((principal_id, display_name));
            }
        }
        // Deterministic order so the fan-out and its tests are stable.
        out.sort();
        Some(out)
    }

    /// True while `principal_id` still holds an active davet for `room` backed by
    /// a live building membership — the delivery-time re-check so a reminder
    /// pending for someone whose davet/membership was revoked after the snapshot
    /// is never delivered.
    pub(in crate::hub) fn everyone_recipient_active(&self, room: &str, principal_id: &str) -> bool {
        let member_tokens: Vec<String> = {
            let invites = self.invites.lock_or_recover();
            let members = self.members.lock_or_recover();
            invites
                .values()
                .filter(|invite| invite.room == room && !invite.member.is_empty())
                .filter(|invite| members.contains_key(&invite.member))
                .map(|invite| invite.member.clone())
                .collect()
        };
        member_tokens.iter().any(|token| {
            self.store.principal_id_for_member_record(token).as_deref() == Some(principal_id)
        })
    }

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
