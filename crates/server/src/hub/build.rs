//! Building the hub at start-up: loading every room, invite, member and record from the store.

use super::{
    broadcast, claims, default_now_ms, groups, loca_name, secret_hash, AdminPairing, Arc,
    AtomicU64, BuildingRole, CareMark, ChatMode, HashMap, Hub, HubConfig, Mutex, Ordering,
    RecoverMutex, Room, RoomSettings, SessionIdentity, Store, BROADCAST_CAP, LOBBY_ROOM,
    NOTIFICATION_SEND_LIMIT,
};

impl Hub {
    /// Full constructor: tokens, a store, default room settings, and a boot
    /// epoch. Loads any persisted rooms from the store on boot.
    pub fn build(
        config: HubConfig,
        store: Arc<Store>,
        default_settings: RoomSettings,
        epoch: u64,
    ) -> Self {
        let HubConfig {
            admin_token,
            room_token,
            require_sessions,
            require_invite,
            home_room,
            reserved_room,
            caretakers,
        } = config;
        let master_name = std::env::var("LOCA_MASTER_NAME")
            .ok()
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "Master".into());
        store
            .ensure_master_principal(&admin_token, &master_name, default_now_ms())
            .expect("bootstrap Master principal");
        let (lobby_tx, _) = broadcast::channel(BROADCAST_CAP);
        let pairing_now = default_now_ms();
        let hub = Hub {
            rooms: Arc::new(Mutex::new(HashMap::new())),
            deleted: Arc::new(Mutex::new(std::collections::HashSet::new())),
            dms: Arc::new(Mutex::new(HashMap::new())),
            operators: Arc::new(Mutex::new(HashMap::new())),
            groups: Arc::new(Mutex::new(groups::GroupBook::default())),
            next_id: Arc::new(AtomicU64::new(1)),
            condition_generation: Arc::new(AtomicU64::new(0)),
            now_ms: default_now_ms,
            admin_token: Arc::new(admin_token),
            room_token: Arc::new(room_token),
            store,
            default_settings,
            epoch,
            members: Arc::new(Mutex::new(HashMap::new())),
            member_keys: Arc::new(Mutex::new(HashMap::new())),
            memory_claims: Arc::new(Mutex::new(claims::MemoryClaims::default())),
            smasters: Arc::new(Mutex::new(HashMap::new())),
            sessions: Arc::new(Mutex::new(HashMap::new())),
            admin_pairing: Arc::new(Mutex::new(AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: pairing_now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
                log_id: None,
            })),
            join_create_times: Arc::new(Mutex::new(std::collections::HashMap::new())),
            require_sessions,
            loca_capacity: Arc::new(std::sync::atomic::AtomicUsize::new(
                Self::DEFAULT_LOCA_CAPACITY,
            )),
            loca_capacity_max: Arc::new(std::sync::atomic::AtomicUsize::new(
                Self::LOCA_CAPACITY_MAX,
            )),
            notification_send_limit: Arc::new(std::sync::atomic::AtomicU32::new(
                NOTIFICATION_SEND_LIMIT,
            )),
            require_invite,
            invites: Arc::new(Mutex::new(HashMap::new())),
            deliveries: Arc::new(Mutex::new(HashMap::new())),
            lobby_tx,
            member_lifecycle: Arc::new(Mutex::new(())),
            name_claims: Arc::new(Mutex::new(())),
            moderation_ranks: Arc::new(Mutex::new(HashMap::new())),
            lobby_online: Arc::new(Mutex::new(HashMap::new())),
            lobby_kits: Arc::new(Mutex::new(HashMap::new())),
            runtime_health: Arc::new(Mutex::new(HashMap::new())),
            care_deliveries: Arc::new(Mutex::new(HashMap::new())),
            home_room: Arc::new(home_room),
            reserved_room: Arc::new(reserved_room),
            caretakers: Arc::new(caretakers),
            #[cfg(test)]
            reissue_swapped_hook: Arc::new(Mutex::new(None)),
        };

        // Invites survive restarts — load them before anyone knocks. An
        // unreadable table stops the start: it must not read as "no invites".
        {
            let mut inv = hub.invites.lock_or_recover();
            for i in hub
                .store
                .load_invites()
                .expect("persistent invites are unreadable; refusing to start")
            {
                inv.insert(i.secret_hash.clone(), i);
            }
            // The deliveries still owed: `Store::open` has already cleared
            // every copy whose window closed while the server was stopped.
            let mut held = hub.deliveries.lock_or_recover();
            for invite_key in hub
                .store
                .load_invite_deliveries()
                .expect("persistent invite deliveries are unreadable; refusing to start")
            {
                let hash = secret_hash(&invite_key);
                if inv.contains_key(&hash) {
                    held.insert(hash, invite_key);
                }
            }
        }

        // Restore persisted state.
        let mut max_id = hub
            .store
            .max_message_id()
            .expect("persistent message high-water mark is unreadable; refusing to start");
        let mut max_generation = 0u64;
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for snap in hub
                .store
                .load()
                .expect("persistent room state is unreadable; refusing to start")
            {
                let mut room = Room::with(snap.mode, snap.settings, snap.max_rev + 1);
                room.history = snap.messages;
                max_generation =
                    max_generation.max(room.history.last().map(|message| message.ts).unwrap_or(0));
                for n in snap.notes {
                    room.notes.insert(n.key.clone(), n);
                }
                for t in hub.store.load_tasks(&snap.room) {
                    max_generation = max_generation.max(t.progress_at);
                    room.next_task_id = room.next_task_id.max(t.id + 1);
                    room.tasks.insert(t.id, t);
                }
                for goal in hub.store.load_goals(&snap.room) {
                    max_generation = max_generation.max(goal.progress_at);
                    room.next_goal_id = room.next_goal_id.max(goal.id + 1);
                    room.goals.insert(goal.id, goal);
                }
                for wait in hub.store.load_waits(&snap.room) {
                    max_generation = max_generation.max(wait.since);
                    room.waits.insert(wait.waiter.clone(), wait);
                }
                for (key, last_signal_at, signal_count) in hub.store.load_care_marks(&snap.room) {
                    room.care_marks.insert(
                        key,
                        CareMark {
                            last_signal_at,
                            signal_count,
                        },
                    );
                }
                for attention in hub.store.attentions(&snap.room) {
                    max_generation = max_generation.max(attention.created_at);
                    if let Some(generation) = attention
                        .id
                        .rsplit(':')
                        .next()
                        .and_then(|part| part.parse::<u64>().ok())
                    {
                        max_generation = max_generation.max(generation);
                    }
                    room.attentions.insert(attention.id.clone(), attention);
                }
                room.last_msg_ms = room.history.last().map(|message| message.ts).unwrap_or(0);
                // The journal is the record of what happened; a restart must
                // not be able to forget it.
                room.journal = hub
                    .store
                    .load_journal(&snap.room)
                    .expect("persistent journal is unreadable; refusing to start");
                room.next_journal_id = room.journal.last().map(|e| e.id + 1).unwrap_or(1);
                rooms.insert(snap.room, room);
                max_id = max_id.max(snap.max_msg_id);
            }
        }
        hub.next_id.store(max_id + 1, Ordering::Relaxed);
        hub.condition_generation
            .store(max_generation, Ordering::Relaxed);
        {
            let mut mem = hub.members.lock_or_recover();
            for m in hub
                .store
                .load_members()
                .expect("persistent memberships are unreadable; refusing to start")
            {
                mem.insert(m.id.clone(), m);
            }
        }
        {
            // Only after `Store::open` committed any registry migration. An
            // unreadable registry stops the start: it must not read as "no
            // Smasters".
            let mut sm = hub.smasters.lock_or_recover();
            for record in hub
                .store
                .load_smasters()
                .expect("persistent Smaster registry is unreadable; refusing to start")
            {
                sm.insert(record.secret_hash.clone(), record);
            }
        }

        // Legacy room settings stored operator display names. Bind only an
        // unambiguous single human label to a principal, then erase the legacy
        // label from settings. Multiple labels, duplicate names, or a missing
        // human are reported and fail closed: migration must never guess who
        // receives authority. Existing principal assignments are authoritative
        // and merely cause the obsolete setting to be cleaned up.
        if hub.store.is_persistent() {
            let master = hub.store.active_master_principal();
            let candidates: Vec<(String, ChatMode, RoomSettings)> = hub
                .rooms
                .lock_or_recover()
                .iter()
                .filter(|(_, room)| !room.settings.operators.is_empty())
                .map(|(name, room)| (name.clone(), room.mode.clone(), room.settings.clone()))
                .collect();
            for (room_name, mode, mut settings) in candidates {
                let existing = hub.store.loca_operator(&room_name);
                let migrated = if existing.is_some() {
                    true
                } else if settings.operators.len() != 1 {
                    tracing::warn!(
                        room = %room_name,
                        legacy_operator_count = settings.operators.len(),
                        "legacy Loca Operator migration needs explicit resolution"
                    );
                    false
                } else {
                    let label = &settings.operators[0];
                    let matches = hub.store.active_human_principals_named(label);
                    if matches.len() != 1 {
                        tracing::warn!(
                            room = %room_name,
                            operator = %label,
                            matching_principals = matches.len(),
                            "legacy Loca Operator label is not uniquely bound; authority withheld"
                        );
                        false
                    } else if let Some(master) = master.as_ref() {
                        match hub.store.appoint_loca_operator(
                            &room_name,
                            &matches[0].id,
                            &master.id,
                            hub.now(),
                        ) {
                            Ok(_) => true,
                            Err(error) => {
                                tracing::error!(
                                    room = %room_name,
                                    operator = %label,
                                    ?error,
                                    "legacy Loca Operator migration failed"
                                );
                                false
                            }
                        }
                    } else {
                        tracing::error!(
                            room = %room_name,
                            "legacy Loca Operator migration has no Master principal"
                        );
                        false
                    }
                };
                if migrated {
                    settings.operators.clear();
                    if let Err(error) = hub.store.save_room(&room_name, &mode, &settings) {
                        tracing::error!(
                            room = %room_name,
                            %error,
                            "failed to clear migrated legacy operator setting"
                        );
                    } else if let Some(room) = hub.rooms.lock_or_recover().get_mut(&room_name) {
                        room.settings = settings;
                    }
                }
            }
        }
        {
            let mut sessions = hub.sessions.lock_or_recover();
            for record in hub
                .store
                .load_admin_sessions(hub.now())
                .expect("persisted admin sessions are unreadable; refusing to start")
            {
                let crate::store::AdminSessionRecord {
                    secret_hash: session_hash,
                    expires_at,
                } = record;
                let Some(principal) = hub
                    .store
                    .principal_for_session_by_hash(&session_hash, hub.now())
                else {
                    // Unbound legacy sessions cannot prove their original role.
                    // Fail closed rather than promoting them to the current Master.
                    hub.store
                        .delete_admin_session_by_hash(&session_hash)
                        .expect("invalid persisted session cleanup failed; refusing to start");
                    continue;
                };
                if !matches!(principal.role, BuildingRole::Master | BuildingRole::Smaster) {
                    hub.store
                        .delete_admin_session_by_hash(&session_hash)
                        .expect("non-admin persisted session cleanup failed; refusing to start");
                    continue;
                }
                sessions.insert(
                    session_hash,
                    SessionIdentity {
                        principal_id: Some(principal.id),
                        building_role: principal.role,
                        name: principal.display_name,
                        kind: principal.kind,
                        loca: None,
                        member: None,
                        invite: None,
                        building_key: false,
                        admin: true,
                        expires_at,
                    },
                );
            }
        }

        // Migration: an invite now seats a MEMBER, but legacy invites predate the
        // link (empty `member_id`). Bind each to the membership carrying its
        // name; when none exists, mint one marked `migration:legacy-invite` —
        // the invite-only identities of the old model become real members, so
        // no one loses a seat they already held. Such a membership's key is
        // never handed to anyone: its holder enters with the invite it holds.
        // After this pass every live invite points at a membership and the new
        // invariant holds building-wide. A write that fails stops the start.
        {
            let mut inv = hub.invites.lock_or_recover();
            let mut mem = hub.members.lock_or_recover();
            let orphans: Vec<String> = inv
                .values()
                .filter(|i| i.member_id.is_empty())
                .map(|i| i.secret_hash.clone())
                .collect();
            for hash in orphans {
                let (name, kind) = {
                    let i = &inv[&hash];
                    (i.name.clone(), i.kind.clone())
                };
                let member_id = match mem.values().find(|m| m.name == name) {
                    Some(m) => m.id.clone(),
                    None => {
                        let key = hub.new_invite_token().replacen("dv_", "mb_", 1);
                        let m = hub
                            .store
                            .add_member(&key, &name, &kind, hub.now(), "migration:legacy-invite")
                            .expect(
                                "binding a legacy invite to a new membership failed; refusing to start",
                            );
                        let id = m.id.clone();
                        mem.insert(id.clone(), m);
                        id
                    }
                };
                hub.store
                    .bind_invite_member_by_hash(&hash, &member_id)
                    .expect("binding a legacy invite to its membership failed; refusing to start");
                if let Some(i) = inv.get_mut(&hash) {
                    i.member_id = member_id;
                }
            }
        }

        // The governance loca is not an ordinary project room. Master and
        // smaster enter by rank; only the two building caretakers may hold
        // invitations. Clean legacy seats at boot so a rename into the
        // reserved loca cannot carry ordinary members across with it.
        if !hub.reserved_room.is_empty() {
            let stale: Vec<String> = hub
                .invites
                .lock_or_recover()
                .values()
                .filter(|invite| {
                    invite.room == *hub.reserved_room && !hub.caretakers.contains(&invite.name)
                })
                .map(|invite| invite.secret_hash.clone())
                .collect();
            for hash in stale {
                hub.store
                    .revoke_invite_by_hash(&hash, hub.now())
                    .expect("persist reserved-loca seat cleanup");
                hub.invites.lock_or_recover().remove(&hash);
            }
        }

        // The configured home room always exists.
        // It may be created here rather than restored from a snapshot (a loca
        // with a journal but no messages leaves no snapshot), so its journal
        // has to be loaded either way — otherwise a restart forgets the record.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            loca_name::require_home_rule(&rooms, &hub.home_room);
            let room = rooms
                .entry((*hub.home_room).clone())
                .or_insert_with(|| Room::with(ChatMode::Free, hub.default_settings.clone(), 1));
            if room.journal.is_empty() {
                room.journal = hub
                    .store
                    .load_journal(&hub.home_room)
                    .expect("persistent home journal is unreadable; refusing to start");
                room.next_journal_id = room.journal.last().map(|e| e.id + 1).unwrap_or(1);
            }
        }

        // The Lobby room always exists: everyone in the Building lands there.
        hub.rooms
            .lock_or_recover()
            .entry(LOBBY_ROOM.to_string())
            .or_insert_with(|| Room::with(ChatMode::Free, hub.default_settings.clone(), 1));

        // Bans and mutes survive the restart (PRINCIPLES: "a restart does not
        // kill the room"). Load them into their rooms so the door is exactly as the
        // operator left it — otherwise a banned name's invite reloads and walks
        // back in.
        {
            let mut rooms = hub.rooms.lock_or_recover();
            for record in hub
                .store
                .load_bans()
                .expect("moderation state is unreadable; refusing to start")
            {
                let crate::store::ModerationRecord {
                    room,
                    name,
                    kind,
                    authority_rank,
                } = record;
                hub.moderation_ranks
                    .lock_or_recover()
                    .insert((room.clone(), name.clone(), kind.clone()), authority_rank);
                let r = Self::room_mut(&mut rooms, &room, &hub.default_settings);
                match kind.as_str() {
                    "ban" => {
                        r.banned.insert(name);
                    }
                    "mute" => {
                        r.muted.insert(name);
                    }
                    _ => {}
                }
            }
        }

        // Sealed locas stay sealed across a restart: their history is kept on
        // disk but they are re-tombstoned so no subscribe/join/post reopens
        // them (PRINCIPLES: seal not destroy, and restart does not undo it).
        {
            let mut deleted = hub.deleted.lock_or_recover();
            for room in hub
                .store
                .sealed_rooms()
                .expect("persistent room seals are unreadable; refusing to start")
            {
                deleted.insert(room);
            }
        }
        {
            let mut dms = hub.dms.lock_or_recover();
            for b in hub
                .store
                .load_dms()
                .expect("persistent direct messages are unreadable; refusing to start")
            {
                dms.insert(b.room.clone(), b);
            }
        }
        // After the legacy operator migration above, so it sees its result.
        *hub.operators.lock_or_recover() = hub
            .store
            .active_loca_operators()
            .expect("persistent operator assignments are unreadable; refusing to start")
            .into_iter()
            .collect();
        hub.load_groups();
        hub
    }
}
