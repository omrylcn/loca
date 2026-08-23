use super::super::*;
use crate::sync::RecoverMutex;

impl Hub {
    pub fn waits(&self, room: &str) -> Vec<WaitState> {
        let rooms = self.rooms.lock_or_recover();
        let mut waits: Vec<WaitState> = rooms
            .get(room)
            .map(|r| r.waits.values().cloned().collect())
            .unwrap_or_default();
        waits.sort_by(|a, b| a.waiter.cmp(&b.waiter));
        waits
    }
    fn wait_cycle_after(room: &Room, proposed: &WaitState) -> Vec<String> {
        let mut path = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut current = proposed.waiter.clone();
        while seen.insert(current.clone()) {
            path.push(current.clone());
            let next = if current == proposed.waiter {
                Some(proposed.waiting_for.clone())
            } else {
                room.waits
                    .get(&current)
                    .map(|wait| wait.waiting_for.clone())
            };
            let Some(next) = next else {
                return Vec::new();
            };
            if next == proposed.waiter {
                return path;
            }
            current = next;
        }
        Vec::new()
    }
    pub(in crate::hub) fn wait_cycles(room: &Room) -> Vec<Vec<String>> {
        let mut names: Vec<&String> = room.waits.keys().collect();
        names.sort();
        let mut seen = std::collections::HashSet::new();
        let mut cycles = Vec::new();
        for name in names {
            let Some(edge) = room.waits.get(name) else {
                continue;
            };
            let cycle = Self::wait_cycle_after(room, edge);
            if !cycle.is_empty() {
                let mut canonical = cycle.clone();
                canonical.sort();
                let identity = canonical.join("\0");
                if seen.insert(identity) {
                    cycles.push(cycle);
                }
            }
        }
        cycles
    }
    /// Declare/replace this participant's explicit dependency edge.
    pub fn set_wait(&self, room: &str, req: SetWait) -> Result<WaitState, WaitError> {
        if req.by == req.waiting_for {
            return Err(WaitError::SelfWait);
        }
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = Self::room_mut(&mut rooms, room, &self.default_settings);
        if let Some(existing) = r.waits.get(&req.by) {
            if existing.waiting_for == req.waiting_for && existing.reason == req.reason {
                return Ok(existing.clone());
            }
        }
        let previous_since = r.waits.get(&req.by).map(|wait| wait.since).unwrap_or(0);
        let wait = WaitState {
            room: room.to_string(),
            waiter: req.by,
            waiting_for: req.waiting_for,
            reason: req.reason,
            since: self.next_condition_generation(now, previous_since),
            last_signal_at: None,
            signal_count: 0,
        };
        self.store
            .replace_wait_with_care(&wait, wait.since)
            .map_err(|_| WaitError::Storage)?;
        let care_key = format!("wait:{}", wait.waiter);
        r.care_marks.remove(&care_key);
        Self::resolve_wait_attentions(r, room, &wait.waiter, wait.since);
        r.waits.insert(wait.waiter.clone(), wait.clone());
        let _ = r.tx.send(ServerFrame::Wait {
            waiter: wait.waiter.clone(),
            wait: Some(wait.clone()),
        });
        Ok(wait)
    }
    pub fn clear_wait(&self, room: &str, waiter: &str) -> Result<(), WaitError> {
        let now = (self.now_ms)();
        let mut rooms = self.rooms.lock_or_recover();
        let r = rooms.get_mut(room).ok_or(WaitError::NotFound)?;
        if !r.waits.contains_key(waiter) {
            return Err(WaitError::NotFound);
        }
        self.store
            .delete_wait(room, waiter, now)
            .map_err(|_| WaitError::Storage)?;
        r.waits.remove(waiter);
        Self::resolve_wait_attentions(r, room, waiter, now);
        let _ = r.tx.send(ServerFrame::Wait {
            waiter: waiter.to_string(),
            wait: None,
        });
        Ok(())
    }
}
