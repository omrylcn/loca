//! Runtime readiness policy and its read-only Building projection.

use super::super::{Hub, RuntimeRecord};
use crate::sync::RecoverMutex;

impl Hub {
    fn runtime_record_ready(&self, record: &RuntimeRecord) -> bool {
        let now = (self.now_ms)();
        if now.saturating_sub(record.seen_at) > Self::RUNTIME_SEEN_TTL_MS {
            return false;
        }
        match record.wake.as_str() {
            "IDLE" => true,
            "OK" => {
                record.ack != "PENDING"
                    || now.saturating_sub(record.progress_at) <= Self::RUNTIME_ACK_GRACE_MS
            }
            "RUNNING" => now.saturating_sub(record.progress_at) <= Self::RUNTIME_WAKE_GRACE_MS,
            _ => false,
        }
    }
    pub(in crate::hub) fn runtime_health_for(&self, name: &str) -> Option<protocol::RuntimeHealth> {
        let record = self.runtime_health.lock_or_recover().get(name).cloned()?;
        Some(protocol::RuntimeHealth {
            wake: record.wake.clone(),
            ack: record.ack.clone(),
            delivery_id: record.delivery_id.clone(),
            attention_id: record.attention_id.clone(),
            stored: record.stored,
            accepted: record.accepted,
            first_response: record.first_response,
            final_response: record.final_response,
            turn_completed: record.turn_completed,
            seen_at: record.seen_at,
            progress_at: record.progress_at,
            ready: self.runtime_record_ready(&record),
        })
    }
    pub fn report_runtime_health(
        &self,
        name: &str,
        update: protocol::RuntimeHealthUpdate,
    ) -> Option<protocol::RuntimeHealth> {
        const STATES: &[&str] = &[
            "IDLE",
            "RUNNING",
            "OK",
            "FAILED",
            "RESTARTING",
            "MANUAL",
            "UNVERIFIED",
        ];
        if !STATES.contains(&update.wake.as_str())
            || !["IDLE", "PENDING", "OK", "UNVERIFIED"].contains(&update.ack.as_str())
            || update
                .delivery_id
                .as_ref()
                .is_some_and(|value| value.len() > 200)
            || update
                .attention_id
                .as_ref()
                .is_some_and(|value| value.len() > 300)
        {
            return None;
        }
        let now = (self.now_ms)();
        let mut health = self.runtime_health.lock_or_recover();
        let changed = health.get(name).is_none_or(|old| {
            old.wake != update.wake
                || old.ack != update.ack
                || old.delivery_id != update.delivery_id
                || old.attention_id != update.attention_id
                || old.stored != update.stored
                || old.accepted != update.accepted
                || old.first_response != update.first_response
                || old.final_response != update.final_response
                || old.turn_completed != update.turn_completed
        });
        let progress_at = if changed {
            now
        } else {
            health.get(name).map(|old| old.progress_at).unwrap_or(now)
        };
        let record = RuntimeRecord {
            wake: update.wake,
            ack: update.ack,
            delivery_id: update.delivery_id,
            attention_id: update.attention_id,
            stored: update.stored,
            accepted: update.accepted,
            first_response: update.first_response,
            final_response: update.final_response,
            turn_completed: update.turn_completed,
            seen_at: now,
            progress_at,
        };
        health.insert(name.to_string(), record.clone());
        drop(health);
        self.runtime_health_for(name)
    }
    /// Everyone the building knows: who they are, which locas they sit in, and
    /// whether they are connected right now.
    ///
    /// This is what makes "call someone in" possible from the UI. Until now the
    /// server only knew who was *in a room*; a member with no seat was
    /// invisible, so the master had nobody to pick from and every invitation
    /// started from scratch — mint a token, hand it over, run setup.
    pub fn residents(&self) -> Vec<protocol::Resident> {
        use std::collections::BTreeMap;
        let mut by_name: BTreeMap<String, protocol::Resident> = BTreeMap::new();

        // Membership IS residency — the list starts here and nothing below
        // adds a name to it. A davet or a live connection can decorate a
        // member with locas/online, but it cannot make someone a resident;
        // that used to happen and it blurred "known to the building" into
        // "was once handed a token", which is how ghosts were born.
        for m in self.members.lock_or_recover().values() {
            by_name.insert(
                m.name.clone(),
                protocol::Resident {
                    name: m.name.clone(),
                    kind: m.kind.clone(),
                    locas: Vec::new(),
                    online: self.lobby_is_online(&m.token),
                    runtime: self.runtime_health_for(&m.name),
                },
            );
        }

        // Davets say where a member currently sits.
        for inv in self.invites.lock_or_recover().values() {
            if let Some(e) = by_name.get_mut(&inv.name) {
                if !e.locas.contains(&inv.room) {
                    e.locas.push(inv.room.clone());
                }
            }
        }

        // A live connection lights a member up — members only; the master and
        // watchers are the building's own, not residents.
        let rooms = self.rooms.lock_or_recover();
        for (room, r) in rooms.iter() {
            for (name, _, _) in r.members.values() {
                if let Some(e) = by_name.get_mut(name) {
                    e.online = true;
                    if !e.locas.contains(room) {
                        e.locas.push(room.clone());
                    }
                }
            }
        }

        by_name.into_values().collect()
    }
}
