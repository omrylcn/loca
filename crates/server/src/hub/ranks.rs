//! The Building's ranks and profiles as people see them, and renaming a profile.

use super::*;

impl Hub {
    pub fn master_principal(&self) -> Option<PrincipalIdentity> {
        self.store.active_master_principal()
    }

    pub fn profiles(&self) -> Vec<PrincipalIdentity> {
        self.store.active_principals()
    }

    /// The Building's ranks — the Master and every Smaster — with the locas
    /// each one is seated in right now. Ranks hold no Lobby membership, so the
    /// resident directory never lists them; this is how the Building view
    /// shows everyone who is part of the Building.
    pub fn building_ranks(&self) -> Vec<RankPresence> {
        let mut ranks: Vec<RankPresence> = Vec::new();
        let mut add = |name: String, role: &'static str, unused: bool| {
            if !ranks.iter().any(|r| r.name == name && r.role == role) {
                ranks.push(RankPresence {
                    name,
                    role,
                    locas: Vec::new(),
                    unused,
                });
            }
        };
        for profile in self.profiles() {
            match profile.role {
                BuildingRole::Master => add(profile.display_name, "master", false),
                BuildingRole::Smaster => {
                    let unused = !self.principal_ever_used(&profile.id);
                    add(profile.display_name, "smaster", unused)
                }
                BuildingRole::Member => {}
            }
        }
        // Memory-only mode persists no profiles; the live registry still knows
        // every Smaster.
        for record in self.smasters.lock_or_recover().values() {
            add(record.name.clone(), "smaster", false);
        }
        let deleted = self.deleted.lock_or_recover().clone();
        let rooms = self.rooms.lock_or_recover();
        let dms = self.dms.lock_or_recover().clone();
        let mut names: Vec<&String> = rooms
            .keys()
            .filter(|r| !deleted.contains(*r) && !dms.contains_key(*r))
            .collect();
        names.sort();
        for room in names {
            for (identity, (label, _, _)) in &rooms[room].members {
                let Some(role) = seat_rank(identity) else {
                    continue;
                };
                // There is one Master, so its seat is its whatever the label;
                // a Smaster seat is matched by the label its session wears.
                let found = ranks
                    .iter_mut()
                    .find(|r| r.role == role && (role == "master" || r.name == *label));
                match found {
                    Some(rank) => rank.locas.push(room.clone()),
                    None => ranks.push(RankPresence {
                        name: label.clone(),
                        role,
                        locas: vec![room.clone()],
                        unused: false,
                    }),
                }
            }
        }
        ranks.sort_by(|a, b| (a.role != "master", &a.name).cmp(&(b.role != "master", &b.name)));
        ranks
    }

    /// `building_ranks` as a viewer may read it: the Master sees every seat;
    /// anyone else never sees a group loca it cannot enter named as "where".
    pub fn building_ranks_for(&self, viewer: &RequestAuthority) -> Vec<RankPresence> {
        let hidden = self.group_locas_hidden_from(viewer);
        let mut ranks = self.building_ranks();
        if !hidden.is_empty() {
            for rank in &mut ranks {
                rank.locas.retain(|room| !hidden.contains(room));
            }
        }
        ranks
    }

    /// A Master or Smaster renames its own profile. Identity, credentials,
    /// sessions and rank stay; only the label changes, everywhere it is worn
    /// live: the Smaster registry, open sessions and every loca seat. Past
    /// messages follow the person: what it said under the old name now reads
    /// the new one, in every loca, and open feeds are told to relabel.
    pub fn rename_profile(
        &self,
        actor: &RequestAuthority,
        requested: &str,
    ) -> Result<PrincipalIdentity, ProfileRenameError> {
        if !actor.is_building_admin() {
            return Err(ProfileRenameError::AuthorityRequired);
        }
        let Some(principal_id) = actor.principal_id.clone() else {
            return Err(ProfileRenameError::AuthorityRequired);
        };
        let name = validate_display_name(requested).map_err(ProfileRenameError::Name)?;
        let current = actor.display_name.clone().unwrap_or_default();
        let _claim = self.name_claims.lock_or_recover();
        let _lifecycle = self.member_lifecycle.lock_or_recover();
        let ranks = self.moderation_ranks.lock_or_recover();
        if ranks
            .iter()
            .any(|((_, restricted_name, _), _)| restricted_name == &current)
        {
            return Err(ProfileRenameError::RestrictionActive);
        }
        let folded = crate::store::search_fold(name);
        let worn_by_other =
            |other: &str| other != current && crate::store::search_fold(other) == folded;
        // A caretaker's name is taken even before that caretaker is admitted:
        // a call to it is relayed to Care (`addressed_caretakers`).
        if self.caretaker_name_folds_to(&folded)
            || self
                .members
                .lock_or_recover()
                .values()
                .any(|m| worn_by_other(&m.name))
            || self
                .smasters
                .lock_or_recover()
                .values()
                .any(|record| record.principal_id != principal_id && worn_by_other(&record.name))
        {
            return Err(ProfileRenameError::NameTaken);
        }
        let relabel = current != name;
        match self.store.rename_principal(&principal_id, name, relabel) {
            Ok(true) => {}
            Ok(false) => return Err(ProfileRenameError::NameTaken),
            Err(_) => return Err(ProfileRenameError::Failed),
        }
        for record in self.smasters.lock_or_recover().values_mut() {
            if record.principal_id == principal_id {
                record.name = name.to_string();
            }
        }
        for session in self.sessions.lock_or_recover().values_mut() {
            if session.principal_id.as_deref() == Some(principal_id.as_str()) {
                session.name = name.to_string();
            }
        }
        let mut seat_keys = vec![format!("sm:pr:{principal_id}")];
        if actor.is_master() {
            seat_keys.push("@master".into());
        }
        // Lock order: name_claims -> member_lifecycle -> moderation_ranks
        // (held since the top) -> rooms, and under rooms only the store's
        // connection, the same order `post` uses when it resolves a reply's
        // author.
        let mut rooms = self.rooms.lock_or_recover();
        for (room_id, room) in rooms.iter_mut() {
            // What older clients are told is exactly what they were told
            // before: `Renamed` when some person's message under the old
            // name is in this tail, judged before anything is relabelled.
            // They relabel by name; the id frame below is what is true.
            let tell_by_name = !current.is_empty()
                && relabel
                && room.history.iter().any(|message| {
                    message.sender_type == SenderType::User && message.sender == current
                });
            if relabel {
                // Exactly this principal's messages, and exactly the replies
                // that answer one of them: ownership is the recorded writer,
                // never the label. A replied-to message that has left the
                // in-memory tail is looked up in storage by id.
                let owns = |message: &Message| {
                    message.sender_principal.as_deref() == Some(principal_id.as_str())
                };
                let owned_here: std::collections::HashSet<u64> = room
                    .history
                    .iter()
                    .filter(|message| owns(message))
                    .map(|message| message.id)
                    .collect();
                let mut ids = Vec::new();
                let mut reply_ids = Vec::new();
                let mut stored_owner: HashMap<u64, bool> = HashMap::new();
                for message in room.history.iter_mut() {
                    if owns(message) && message.sender != name {
                        message.sender = name.to_string();
                        ids.push(message.id);
                    }
                    let (Some(label), Some(target)) =
                        (message.reply_to_sender.as_deref(), message.reply_to)
                    else {
                        continue;
                    };
                    if label == name {
                        continue;
                    }
                    let answers_renamer = if owned_here.contains(&target) {
                        true
                    } else if let Some(&known) = stored_owner.get(&target) {
                        known
                    } else {
                        match self.store.message_sender_principal(room_id, target) {
                            Ok(writer) => {
                                let owned = writer.as_deref() == Some(principal_id.as_str());
                                stored_owner.insert(target, owned);
                                owned
                            }
                            Err(error) => {
                                tracing::warn!(%error, room = %room_id, message = message.id,
                                    "reply label left unchanged: replied-to writer unreadable");
                                false
                            }
                        }
                    };
                    if answers_renamer {
                        message.reply_to_sender = Some(name.to_string());
                        reply_ids.push(message.id);
                    }
                }
                if !ids.is_empty() || !reply_ids.is_empty() {
                    let _ = room.tx.send(ServerFrame::Relabelled {
                        to: name.to_string(),
                        ids,
                        reply_ids,
                    });
                }
            }
            if tell_by_name {
                let _ = room.tx.send(ServerFrame::Renamed {
                    from: current.clone(),
                    to: name.to_string(),
                });
            }
            let mut touched = false;
            for key in &seat_keys {
                if let Some(seat) = room.members.get_mut(key) {
                    seat.0 = name.to_string();
                    touched = true;
                }
            }
            if touched {
                let members = room.member_list(&self.profile_cards());
                let _ = room.tx.send(ServerFrame::Members { members });
            }
        }
        drop(rooms);
        Ok(self
            .store
            .active_principal(&principal_id)
            .unwrap_or(PrincipalIdentity {
                id: principal_id,
                display_name: name.to_string(),
                kind: SenderType::User,
                role: actor.building_role.unwrap_or(BuildingRole::Member),
            }))
    }
}
