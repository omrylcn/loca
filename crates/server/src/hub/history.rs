//! Reading a loca's words: recent messages, pages after an id, its members and the room list.

use super::*;

impl Hub {
    /// Room memory search: full message archive (DB) + current notes.
    pub fn search(
        &self,
        room: &str,
        q: &str,
        limit: usize,
    ) -> rusqlite::Result<(Vec<Message>, Vec<Note>)> {
        let msgs = self.store.search_messages(room, q, limit)?;
        let ql = crate::store::search_fold(q);
        let notes: Vec<Note> = {
            let rooms = self.rooms.lock_or_recover();
            rooms
                .get(room)
                .map(|r| {
                    r.notes
                        .values()
                        .filter(|n| {
                            crate::store::search_fold(&n.title).contains(&ql)
                                || crate::store::search_fold(&n.body).contains(&ql)
                                || crate::store::search_fold(&n.key).contains(&ql)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        Ok((msgs, notes))
    }

    /// The bounded hot tail used by a normal browser room-open.
    pub fn recent_messages(&self, room: &str) -> Vec<Message> {
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.history.clone())
            .unwrap_or_default()
    }

    /// One ordered page after `since` (exclusive) for durable REST backfill.
    pub fn messages_after(
        &self,
        room: &str,
        since: u64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        self.messages_after_within(room, since, 0, u64::MAX, limit)
    }

    /// [`Hub::messages_after`], leaving out everything said before `from_ts`
    /// and everything with an id above `through_id`.
    pub(in crate::hub) fn messages_after_within(
        &self,
        room: &str,
        since: u64,
        from_ts: u64,
        through_id: u64,
        limit: usize,
    ) -> rusqlite::Result<Vec<Message>> {
        if let Some(messages) = self
            .store
            .messages_after(room, since, from_ts, through_id, limit)?
        {
            return Ok(messages);
        }
        let rooms = self.rooms.lock_or_recover();
        Ok(rooms
            .get(room)
            .map(|r| {
                r.history
                    .iter()
                    .filter(|m| m.id > since && m.ts >= from_ts && m.id <= through_id)
                    .take(limit)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    pub fn members(&self, room: &str) -> Vec<Member> {
        let cards = self.profile_cards();
        let rooms = self.rooms.lock_or_recover();
        rooms
            .get(room)
            .map(|r| r.member_list(&cards))
            .unwrap_or_default()
    }

    /// Loca list for a caller. An invite opens ONE loca — so the list an invite
    /// holder sees is that one loca, not the whole building's floor plan (which
    /// leaked every loca's name, member count, mode and last line to anyone
    /// holding any invite). `reach` returns true for a loca the caller may
    /// enter; the master/building-key reaches all, an invite reaches its own.
    pub fn room_summaries_for(&self, reach: impl Fn(&str) -> bool) -> Vec<RoomSummary> {
        // Decide reachability BEFORE taking the rooms lock: `reach` calls
        // `enter_decision`, which itself locks `rooms` (via is_banned). Calling
        // it while holding the lock here would deadlock the same mutex.
        let names: Vec<String> = {
            let rooms = self.rooms.lock_or_recover();
            rooms.keys().cloned().collect()
        };
        let visible: std::collections::HashSet<String> =
            names.into_iter().filter(|n| reach(n)).collect();
        let groups = self.group_names_by_room();

        let rooms = self.rooms.lock_or_recover();
        let mut list: Vec<RoomSummary> = rooms
            .iter()
            .filter(|(name, _)| visible.contains(name.as_str()))
            .map(|(name, r)| {
                let humans = r
                    .members
                    .values()
                    .filter(|(_, k, n)| !n.is_empty() && *k == SenderType::User)
                    .count();
                let agents = r
                    .members
                    .values()
                    .filter(|(_, k, n)| !n.is_empty() && *k == SenderType::Agent)
                    .count();
                RoomSummary {
                    room: name.clone(),
                    members: humans + agents,
                    humans,
                    agents,
                    mode: r.mode.clone(),
                    archived: r.settings.archived,
                    last: r.history.last().map(|m| {
                        let t: String = m.text.chars().take(48).collect();
                        format!("{}: {}", m.sender, t)
                    }),
                    last_id: r.history.last().map(|m| m.id).unwrap_or(0),
                    special: self.is_reserved_room(name),
                    group: groups.get(name).map(|(_, group)| group.clone()),
                    group_id: groups.get(name).map(|(id, _)| id.clone()),
                    // Only callers `reach` lets in see these, like the rest.
                    title: r.settings.title.clone(),
                    description: r.settings.description.clone(),
                    icon: r.settings.icon.clone(),
                    color: r.settings.color.clone(),
                    labels: r.settings.labels.clone(),
                }
            })
            .collect();
        list.sort_by(|a, b| a.room.cmp(&b.room));
        list
    }
}
