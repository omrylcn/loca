//! Ways into the Building: the Master's pairing code, admission rights and join requests.

use super::{
    admission_name_refusal, AdminPairing, Approve, ChatMode, Hub, JoinRequestCreate, Message,
    Ordering, RecoverMutex, SenderType, ServerFrame,
};

impl Hub {
    /// The boot-time pairing code is printed only by the server process. It is
    /// a one-use stand-in, never the root key itself.
    pub fn admin_pairing_code(&self) -> Option<String> {
        if self.admin_token.is_empty() {
            None
        } else {
            let current = self.admin_pairing.lock_or_recover();
            ((self.now_ms)() < current.expires_at_ms).then(|| current.code.clone())
        }
    }

    /// Rotate a pairing code and bind the operator-selected session lifetime
    /// to that exact one-use code. Validation belongs at the HTTP boundary;
    /// clamping here is defence in depth for internal callers.
    pub fn rotate_admin_pairing_for(&self, session_ttl_ms: u64) -> Option<(String, u64)> {
        if self.admin_token.is_empty() {
            return None;
        }
        let now = (self.now_ms)();
        let expires_at_ms = now.saturating_add(Self::ADMIN_PAIRING_TTL_MS);
        let mut current = self.admin_pairing.lock_or_recover();
        // Only one Master code works at a time: an unused earlier one is
        // superseded, and the ledger says so.
        if let Some(previous) = current.log_id.take() {
            let _ = self.store.mark_pairing_superseded(&previous, now);
        }
        let log_id = Self::secure_token("pl_", 12);
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: session_ttl_ms.clamp(
                Self::MIN_ADMIN_SESSION_TTL_MS,
                Self::MAX_ADMIN_SESSION_TTL_MS,
            ),
            expires_at_ms,
            log_id: Some(log_id.clone()),
        };
        if let Err(error) = self.store.log_pairing(
            &log_id,
            &current.code,
            now,
            expires_at_ms,
            current.session_ttl_ms,
        ) {
            tracing::error!(%error, "could not record the Master code in the key ledger");
        }
        Some((current.code.clone(), current.expires_at_ms))
    }

    /// Consume the current browser pairing code exactly once. Comparison walks
    /// the full fixed-length value so a remote caller gets no useful prefix
    /// timing signal.
    pub fn consume_admin_pairing(&self, candidate: &str) -> Option<u64> {
        if self.admin_token.is_empty() {
            return None;
        }
        let mut current = self.admin_pairing.lock_or_recover();
        let now = (self.now_ms)();
        if now >= current.expires_at_ms {
            *current = AdminPairing {
                code: Self::secure_token("pair_", 16),
                session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
                expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
                log_id: None,
            };
            return None;
        }
        let a = current.code.as_bytes();
        let b = candidate.as_bytes();
        let mut difference = a.len() ^ b.len();
        for index in 0..a.len().max(b.len()) {
            difference |= usize::from(
                a.get(index).copied().unwrap_or(0) ^ b.get(index).copied().unwrap_or(0),
            );
        }
        if difference != 0 {
            return None;
        }
        let session_ttl_ms = current.session_ttl_ms;
        if let Some(id) = current.log_id.as_deref() {
            let _ = self.store.mark_pairing_used(id, now);
        }
        *current = AdminPairing {
            code: Self::secure_token("pair_", 16),
            session_ttl_ms: Self::ADMIN_SESSION_TTL_MS,
            expires_at_ms: now.saturating_add(Self::ADMIN_PAIRING_TTL_MS),
            log_id: None,
        };
        tracing::info!("master browser paired; next one-use pairing code is ready");
        Some(session_ttl_ms)
    }

    /// A rank pre-mints `count` single-use, time-limited Lobby-admission
    /// rights. Returns `(minted, total_ever, available_now)`. Rank is enforced
    /// at the route; `minted_by` is the principal that minted them, or the
    /// Master principal when the caller has none (the Master desk, the root
    /// credential). The right ids are server-generated CSPRNG tokens and are
    /// NOT returned — a right is delivered only when the join-request approve
    /// step consumes it.
    pub fn mint_admission_stock(
        &self,
        count: u32,
        ttl_ms: u64,
        minted_by: Option<&str>,
    ) -> (u64, u64, u64) {
        let now = (self.now_ms)();
        let expires_at = now.saturating_add(ttl_ms);
        let minted_by = minted_by.map(str::to_string).unwrap_or_else(|| {
            self.store
                .active_master_principal()
                .map(|principal| principal.id)
                .unwrap_or_else(|| "master".to_string())
        });
        let ids: Vec<String> = (0..count).map(|_| Self::secure_token("adm_", 24)).collect();
        let minted = self
            .store
            .mint_admission_rights(&ids, &minted_by, now, expires_at)
            .unwrap_or(0) as u64;
        let (total, available) = self.store.admission_stock_counts(now);
        (minted, total, available)
    }

    /// `(total_ever, available_now)` admission-stock summary for the Master view.
    pub fn admission_stock_summary(&self) -> (u64, u64) {
        self.store.admission_stock_counts((self.now_ms)())
    }

    /// Per-source (peer IP) sliding-window limit on join-request creation (review
    /// re-blocker #3): one abusive source is throttled on its own bucket without
    /// affecting anyone else. Returns true, and records the attempt, when a create
    /// is allowed for `ip`. Empty buckets are reaped so the map cannot grow
    /// unbounded.
    pub(in crate::hub) fn join_request_rate_ok(&self, ip: std::net::IpAddr) -> bool {
        let now = (self.now_ms)();
        let mut by_ip = self.join_create_times.lock_or_recover();
        by_ip.retain(|_, times| {
            times.retain(|&t| now.saturating_sub(t) < Self::JOIN_CREATE_WINDOW_MS);
            !times.is_empty()
        });
        let times = by_ip.entry(ip).or_default();
        if times.len() >= Self::JOIN_CREATE_MAX_IN_WINDOW {
            return false;
        }
        times.push(now);
        true
    }

    /// An outside agent asks to join, picking its own name. The plaintext secret
    /// is produced exactly once here and only its hash is stored, so only the
    /// requester can poll or bootstrap it. Grants NOTHING until a Master approves.
    /// The name must be free — neither an existing member nor an already-pending
    /// request — which (with the approve-time atomic re-check) closes the identity
    /// takeover and the two-same-name race (review blocker #1). Rate-limited per
    /// source IP.
    pub fn create_join_request(
        &self,
        name: &str,
        kind: &str,
        ip: std::net::IpAddr,
    ) -> JoinRequestCreate {
        if !self.join_request_rate_ok(ip) {
            return JoinRequestCreate::BacklogFull;
        }
        // Checked and filed under `name_claims`: a pending request holds its
        // name until it is decided. Released before the announcement.
        let claim = self.name_claims.lock_or_recover();
        if self.member_by_name(name).is_some() || self.store.has_pending_join_request_named(name) {
            return JoinRequestCreate::NameTaken;
        }
        if self.new_member_name_clashes(name) {
            return JoinRequestCreate::NameWorn;
        }
        if self.store.list_pending_join_requests().len() >= Self::MAX_PENDING_JOIN_REQUESTS {
            return JoinRequestCreate::BacklogFull;
        }
        let request_id = Self::secure_token("jr_", 12);
        let request_secret = Self::secure_token("jrs_", 24);
        let _ = self.store.create_join_request(
            &request_id,
            &request_secret,
            name,
            kind,
            (self.now_ms)(),
        );
        drop(claim);
        // Make the request VISIBLE where a human can act on it: a persisted
        // announcement in the home loca so the Master/Smaster/loca-care see
        // "<name> wants to join" in chat (addressed to the lead so a filtered
        // runtime also wakes). Without this the request only reached the hidden
        // Master Desk and never surfaced to the operator.
        self.announce_join_request(name, kind);
        JoinRequestCreate::Created {
            request_id,
            request_secret,
        }
    }

    /// Post a visible "<name> wants to join" announcement into the home loca so
    /// the request surfaces in the main app (not only the admin desk). Persisted
    /// and broadcast, addressed to the lead (falling back to `all`) so a filtered
    /// runtime wakes. Never carries any secret — just the requested name/kind.
    pub(in crate::hub) fn announce_join_request(&self, name: &str, kind: &str) {
        let room = self.home_room.as_ref().clone();
        let mut rooms = self.rooms.lock_or_recover();
        // Never silently drop the announce (review blocker): if the home loca is
        // not in memory yet, use defaults and let `room_mut` materialise it — the
        // same shape `set_lead` uses. And persist BEST-EFFORT: even if the DB
        // write fails, still push to history + broadcast so the request always
        // surfaces live. The request row already exists; the only job here is to
        // make it visible, and that must not be conditional.
        let (mode, settings) = rooms
            .get(&room)
            .map(|r| (r.mode.clone(), r.settings.clone()))
            .unwrap_or_else(|| (ChatMode::Free, self.default_settings.clone()));
        let target = settings.lead.clone().or_else(|| Some("all".into()));
        let msg = Message {
            attachments: Vec::new(),
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            room: room.clone(),
            sender: "loca".into(),
            sender_type: SenderType::User,
            target,
            text: format!("{name} ({kind}) wants to join — approve or deny it in Join requests."),
            reply_to: None,
            reply_to_sender: None,
            sender_principal: None,
            kind: protocol::MessageKind::Announce,
            ts: (self.now_ms)(),
        };
        let _ = self
            .store
            .insert_message_with_room(&msg, &mode, &settings, None);
        let r = Self::room_mut(&mut rooms, &room, &self.default_settings);
        r.history.push(msg.clone());
        let _ = r.tx.send(ServerFrame::Msg { message: msg });
    }

    /// Poll a join request with its secret. `Ok(None)`: unknown request or
    /// wrong secret; a storage error is an error.
    pub fn join_request_view(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<Option<crate::store::JoinRequestView>> {
        self.store
            .join_request_view(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not read a join request"))
    }

    /// The Master's pending-request review list.
    pub fn list_pending_join_requests(&self) -> Vec<(String, String, String, u64)> {
        self.store.list_pending_join_requests()
    }

    /// The name a still-pending join request asks for (it never changes).
    pub fn pending_join_request_name(&self, id: &str) -> Option<String> {
        self.store
            .list_pending_join_requests()
            .into_iter()
            .find(|(request, ..)| request == id)
            .map(|(_, name, ..)| name)
    }

    /// Approve a pending join request (Master action). EXACTLY-ONCE: atomically
    /// claims the request first; a repeated or racing approve that finds it
    /// already decided returns `Approve::AlreadyDecided` and consumes NO stock.
    /// On a fresh claim it consumes one admission-stock right and issues a Lobby
    /// membership (`mb_`, kind `agent` per the model) bound to the requested name;
    /// if the stock is empty it releases the claim and returns `Approve::NoStock`.
    pub fn approve_join_request(&self, id: &str, by: &str) -> Approve {
        // The ENTIRE approval — pending-guard, name-free check, stock consume,
        // member insert, request-finalize — happens in ONE store transaction
        // (`approve_join_request_atomic`). This closes the identity-takeover race
        // (no `/members` create can slip between the name check and the insert:
        // the single conn Mutex is held throughout) AND removes every partial
        // state: a failure at any step leaves the request pending, the name free,
        // and the stock intact — so there is no compensating refund/release to
        // orchestrate here, and no way to strand a request or burn a right.
        // We only mint the fresh `mb_` token up front (needs the Hub's CSPRNG).
        let mb_token = self.new_invite_token().replacen("dv_", "mb_", 1);
        // Approving admits a new name, so it meets the doors' rule again: a
        // rank or a caretaker may have come to wear it since the request was
        // filed, or an older server filed it unchecked. Refused here, before
        // the transaction, nothing is written: the request stays pending (the
        // Master can still deny it) and no admission right is consumed. Held
        // under `name_claims` until the new member is in the cache too, so a
        // member, a Smaster or a rename racing this approval sees it or is
        // seen by it.
        let _claim = self.name_claims.lock_or_recover();
        if let Some(name) = self.pending_join_request_name(id) {
            if let Some(rule) = admission_name_refusal(&name) {
                return Approve::NameRule(rule);
            }
            if self.member_by_name(&name).is_none() && self.new_member_name_clashes(&name) {
                return Approve::NameWorn;
            }
        }
        match self
            .store
            .approve_join_request_atomic(id, &mb_token, by, (self.now_ms)())
        {
            crate::store::ApproveTxn::Committed(member) => {
                // Mirror the committed DB row into the in-memory member cache.
                self.members
                    .lock_or_recover()
                    .insert(member.id.clone(), member);
                Approve::Approved
            }
            crate::store::ApproveTxn::AlreadyDecided => Approve::AlreadyDecided,
            crate::store::ApproveTxn::NameTaken => Approve::NameTaken,
            crate::store::ApproveTxn::NoStock => Approve::NoStock,
            crate::store::ApproveTxn::Failed => Approve::Failed,
        }
    }

    /// Deny a pending join request (Master action). True iff it was pending.
    pub fn deny_join_request(&self, id: &str, by: &str) -> bool {
        self.store.deny_join_request(id, by, (self.now_ms)())
    }

    /// Deliver an approved request's `mb_`, re-fetchable until the client ACKs
    /// (crash-safe: the credential cannot be lost between receive and persist)
    /// or until `store::JOIN_BOOTSTRAP_WINDOW_MS` after the approval.
    pub fn claim_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<crate::store::BootstrapClaim> {
        self.store
            .claim_join_request_bootstrap(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not read a join request's key"))
    }

    /// Finalize an approved request's bootstrap after the client verified its
    /// `mb_` via `/whoami`. Closes the re-fetch window. `Some(true)` = just
    /// acked, `Some(false)` = already acked by this secret, `None` = unknown /
    /// unowned / not approved. Also answers after the delivery deadline: a
    /// requester whose ACK was lost has done nothing wrong.
    pub fn ack_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
    ) -> rusqlite::Result<Option<bool>> {
        self.store
            .ack_join_request_bootstrap(id, secret, (self.now_ms)())
            .inspect_err(|error| tracing::error!(%error, "could not acknowledge a join request"))
    }

    /// Clear key delivery copies whose window has closed (join requests and
    /// membership claims), and retry an owed WAL clean-up. Called by the
    /// background sweep, so a copy nobody asks for again still ends on time.
    /// The membership and the claimed credentials are not touched. A storage
    /// error is logged as one, never as "nothing to clear".
    pub fn expire_join_request_bootstraps(&self) {
        let memory = self.expire_memory_claim_copies();
        match self.store.expire_join_request_bootstraps((self.now_ms)()) {
            Ok(0) if memory == 0 => {}
            Ok(cleared) => tracing::info!(
                cleared = cleared + memory,
                "key delivery copies (join requests, membership claims) past their window were cleared"
            ),
            Err(error) => tracing::error!(
                %error,
                "could not clear key delivery copies past their window; none cleared"
            ),
        }
    }
}
