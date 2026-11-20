//! Ways into the Building on disk: admission rights, join requests and their
//! bootstrap window, and invite delivery copies.

#[cfg(test)]
use super::migrations::checkpoint_owed;
use super::{
    expire_delivery_copies, expire_invite_copies, identity, params, record_owed_checkpoint,
    secret_hash, settle_owed_checkpoint_logged, with_secure_delete, ApproveTxn, BootstrapClaim,
    DeliveryCopies, JoinRequestView, MemberRecord, OptionalExtension, Store,
    JOIN_BOOTSTRAP_WINDOW_MS, PAST_JOIN_BOOTSTRAP_DEADLINE,
};

impl Store {
    /// Insert one admission-stock row per pre-generated right id (a Master
    /// pre-minting a batch of single-use Lobby-admission rights). All-or-nothing
    /// in one transaction, and returns the number of rows actually inserted so
    /// the caller reports a truthful `minted` count (never the merely-requested
    /// one) even if the batch is rolled back.
    pub fn mint_admission_rights(
        &self,
        ids: &[String],
        minted_by: &str,
        created_at: u64,
        expires_at: u64,
    ) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
        let tx = c.transaction()?;
        let mut inserted = 0usize;
        for id in ids {
            inserted += tx.execute(
                "INSERT INTO admission_stock (id, minted_by, created_at, expires_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![id, minted_by, created_at, expires_at],
            )?;
        }
        tx.commit()?;
        Ok(inserted)
    }

    /// `(total_ever, available_now)` where available = unconsumed AND unexpired.
    pub fn admission_stock_counts(&self, now: u64) -> (u64, u64) {
        let Some(c) = self.conn() else { return (0, 0) };
        let total: u64 = c
            .query_row("SELECT count(*) FROM admission_stock", [], |r| r.get(0))
            .unwrap_or(0);
        let available: u64 = c
            .query_row(
                "SELECT count(*) FROM admission_stock \
                 WHERE consumed_at IS NULL AND expires_at > ?1",
                params![now],
                |r| r.get(0),
            )
            .unwrap_or(0);
        (total, available)
    }

    /// Record a new pending join request. The caller's request-secret is stored
    /// only as a hash, so only the requester (who holds the plaintext) can later
    /// poll or bootstrap it.
    pub fn create_join_request(
        &self,
        id: &str,
        secret: &str,
        name: &str,
        kind: &str,
        created_at: u64,
    ) -> rusqlite::Result<()> {
        let Some(c) = self.conn() else { return Ok(()) };
        c.execute(
            "INSERT INTO join_requests (id, secret_hash, name, kind, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
            params![id, secret_hash(secret), name, kind, created_at],
        )
        .map(|_| ())
    }

    /// What the requester may know about its own request, if `id` + `secret`
    /// match; `Ok(None)` otherwise (a caller learns nothing about a request
    /// that is not theirs). A storage error is an error, never "no such
    /// request": the requester's helper forgets its request when told it does
    /// not exist, so a passing database fault must not read as that.
    pub fn join_request_view(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<Option<JoinRequestView>> {
        let Some(c) = self.conn() else {
            return Ok(None);
        };
        // `bootstrap_ready` stays true from approval until the client ACKs, so a
        // requester that restarts mid-flow re-discovers it and re-bootstraps —
        // gated on `bootstrap_acked_at`, not first delivery — but never past
        // the delivery deadline, even before anything has cleared the copy.
        // `bootstrap_expired` is never true once the ACK is in: an acknowledged
        // request ended normally, whatever the clock says afterwards.
        c.query_row(
            &format!(
                "SELECT status, name, \
                        (mb_token IS NOT NULL AND bootstrap_acked_at IS NULL \
                         AND NOT {past}), \
                        (status = 'approved' AND bootstrap_acked_at IS NULL \
                         AND (bootstrap_expired_at IS NOT NULL \
                              OR (mb_token IS NOT NULL AND {past}))) \
                 FROM join_requests WHERE id = :id AND secret_hash = :secret_hash",
                past = PAST_JOIN_BOOTSTRAP_DEADLINE
            ),
            rusqlite::named_params! {
                ":id": id,
                ":secret_hash": secret_hash(secret),
                ":now": now,
                ":window": JOIN_BOOTSTRAP_WINDOW_MS,
            },
            |r| {
                Ok(JoinRequestView {
                    status: r.get(0)?,
                    name: r.get(1)?,
                    bootstrap_ready: r.get(2)?,
                    bootstrap_expired: r.get(3)?,
                })
            },
        )
        .optional()
    }

    /// Pending requests for the Master review list (id, name, kind, created_at);
    /// never any secret or token.
    pub fn list_pending_join_requests(&self) -> Vec<(String, String, String, u64)> {
        let Some(c) = self.conn() else {
            return Vec::new();
        };
        let Ok(mut stmt) = c.prepare(
            "SELECT id, name, kind, created_at FROM join_requests \
             WHERE status = 'pending' ORDER BY created_at, id",
        ) else {
            return Vec::new();
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, u64>(3)?,
            ))
        }) else {
            return Vec::new();
        };
        rows.flatten().collect()
    }

    /// Is there already a live (pending or being-approved) request for this
    /// name, in any letter case? Keeps names unique across live requests so two
    /// requesters can never both be approved into the same name, or into two
    /// names one `@mention` would call together.
    pub fn has_pending_join_request_named(&self, name: &str) -> bool {
        let Some(c) = self.conn() else { return false };
        c.query_row(
            "SELECT 1 FROM join_requests \
             WHERE lower(name) = lower(?1) AND status IN ('pending', 'approving') LIMIT 1",
            params![name],
            |_| Ok(()),
        )
        .is_ok()
    }

    /// Approve a pending join request as ONE transaction: guard that it is still
    /// pending, verify the name is free, consume exactly one available admission
    /// right, insert the new Lobby member, and mark the request approved carrying
    /// its `mb_`. Because the single `Mutex<Connection>` is held for the whole
    /// transaction nothing can interleave (no `/members` insert slips between the
    /// name check and the member insert), and because it is one commit there is
    /// NO partial state: a crash or any failed step leaves the request `pending`,
    /// the name free, and the stock intact — no leaked right, no request stranded
    /// in an intermediate `approving` state (which `claim` used to make
    /// unrecoverable). It never touches an existing member, so a foreign `mb_`
    /// can never be handed out. Returns the created membership's record so the
    /// caller can mirror it into the in-memory roster.
    pub fn approve_join_request_atomic(
        &self,
        request_id: &str,
        mb_token: &str,
        by: &str,
        now: u64,
    ) -> ApproveTxn {
        let Some(mut c) = self.conn() else {
            return ApproveTxn::Failed;
        };
        let tx = match c.transaction() {
            Ok(tx) => tx,
            Err(_) => return ApproveTxn::Failed,
        };

        // 1) The request must still be pending. Flip it to `approving` INSIDE the
        //    txn so a concurrent approve of the same id finds nothing to claim;
        //    the flip is invisible outside the transaction and is undone by any
        //    rollback below, so no durable `approving` state is ever observable.
        match tx.execute(
            "UPDATE join_requests SET status = 'approving' \
             WHERE id = ?1 AND status = 'pending'",
            params![request_id],
        ) {
            Ok(1) => {}
            Ok(_) => {
                let _ = tx.rollback();
                return ApproveTxn::AlreadyDecided;
            }
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
        }

        // The requester's chosen name + kind (immutable since creation).
        let (name, kind): (String, String) = match tx.query_row(
            "SELECT name, kind FROM join_requests WHERE id = ?1",
            params![request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ) {
            Ok(v) => v,
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
        };

        // 2) The name must be free, in any letter case (a member admitted
        //    directly since the request was filed may wear it). Checked and
        //    inserted under the same held connection, so no `/members` create
        //    can slip in between.
        if tx
            .query_row(
                "SELECT 1 FROM members \
                 WHERE lower(name) = lower(?1) AND revoked_at IS NULL LIMIT 1",
                params![name],
                |_| Ok(()),
            )
            .is_ok()
        {
            let _ = tx.rollback(); // undoes the 'approving' flip -> back to pending
            return ApproveTxn::NameTaken;
        }

        // 3) Consume exactly one available, unexpired admission right (oldest
        //    first). The `consumed_at IS NULL` guard keeps the write single-use.
        let right_id: String = match tx.query_row(
            "SELECT id FROM admission_stock \
             WHERE consumed_at IS NULL AND expires_at > ?1 \
             ORDER BY created_at, id LIMIT 1",
            params![now],
            |r| r.get(0),
        ) {
            Ok(id) => id,
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::NoStock;
            }
        };
        match tx.execute(
            "UPDATE admission_stock SET consumed_at = ?1, consumed_by_name = ?2 \
             WHERE id = ?3 AND consumed_at IS NULL",
            params![now, name, right_id],
        ) {
            Ok(1) => {}
            _ => {
                let _ = tx.rollback();
                return ApproveTxn::NoStock;
            }
        }

        // 4) Insert the fresh Lobby member — into `members` AND the identity-v2
        //    principals/credentials tables (exactly what `add_member` does), so
        //    the issued `mb_` authenticates IMMEDIATELY on a persistent store.
        //    Writing only `members` here made the credential resolve only after
        //    the next restart-time migration, so a freshly-approved agent got
        //    401 at the Lobby until a restart (review blocker). The member row
        //    keeps the key's hash and ids, never the key.
        let ids = identity::MemberIds::from_key(mb_token);
        let mut member = MemberRecord {
            id: ids.id.clone(),
            secret_hash: ids.secret_hash.clone(),
            principal_id: None,
            name,
            kind,
            joined_at: now,
            admitted_by: by.to_string(),
        };
        match identity::insert_member_record(&tx, "members", &ids, &member, None, "Member access") {
            Ok(principal_id) => member.principal_id = Some(principal_id),
            Err(_) => {
                let _ = tx.rollback();
                return ApproveTxn::Failed;
            }
        }

        // 5) Mark the request approved, carrying the mb_ for one-time bootstrap.
        //    (Split from the commit so the guard never moves `tx`.)
        if !matches!(
            tx.execute(
                "UPDATE join_requests \
                 SET status = 'approved', mb_token = ?2, decided_by = ?3, decided_at = ?4 \
                 WHERE id = ?1 AND status = 'approving'",
                params![request_id, mb_token, by, now],
            ),
            Ok(1)
        ) {
            let _ = tx.rollback();
            return ApproveTxn::Failed;
        }
        if tx.commit().is_ok() {
            ApproveTxn::Committed(member)
        } else {
            // commit failed — nothing is persisted; the request stays pending.
            ApproveTxn::Failed
        }
    }

    /// Deny a pending request. Returns true iff it was pending (now denied).
    pub fn deny_join_request(&self, id: &str, decided_by: &str, decided_at: u64) -> bool {
        let Some(c) = self.conn() else { return false };
        c.execute(
            "UPDATE join_requests SET status = 'denied', decided_by = ?2, decided_at = ?3 \
             WHERE id = ?1 AND status = 'pending'",
            params![id, decided_by, decided_at],
        )
        .map(|n| n == 1)
        .unwrap_or(false)
    }

    /// Deliver the issued mb_, RE-FETCHABLE until the client ACKs or the
    /// delivery deadline passes. It returns the same mb_ on every call while
    /// the request is approved, secret-matched, not-yet-acked and inside
    /// `JOIN_BOOTSTRAP_WINDOW_MS` of its approval, so a client that crashes or
    /// drops the connection between receiving mb_ and persisting it can simply
    /// ask again — the credential is never lost inside the window.
    /// `bootstrap_delivered_at` records the FIRST delivery time for
    /// observability only; it does not gate re-delivery.
    ///
    /// A call at or after the deadline gets `Expired` and clears the copy in
    /// the same call (see `expire_delivery_copies`), so a late caller is the
    /// last one who could have asked. `Unavailable` covers everything a caller
    /// may not learn more about: unknown id, wrong secret, not approved, or
    /// already acknowledged. A storage error is returned as an error, never as
    /// either of those answers and never as a delivery.
    pub fn claim_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<BootstrapClaim> {
        let Some(mut c) = self.conn() else {
            return Ok(BootstrapClaim::Unavailable);
        };
        let row: Option<(Option<String>, bool, bool, bool, bool)> = c
            .query_row(
                &format!(
                    "SELECT mb_token, bootstrap_acked_at IS NOT NULL, \
                            bootstrap_expired_at IS NOT NULL, {PAST_JOIN_BOOTSTRAP_DEADLINE}, \
                            bootstrap_delivered_at IS NOT NULL \
                     FROM join_requests \
                     WHERE id = :id AND secret_hash = :secret_hash AND status = 'approved'"
                ),
                rusqlite::named_params! {
                    ":id": id,
                    ":secret_hash": secret_hash(secret),
                    ":now": now,
                    ":window": JOIN_BOOTSTRAP_WINDOW_MS,
                },
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let Some((mb, acked, cleared, past, delivered)) = row else {
            return Ok(BootstrapClaim::Unavailable);
        };
        if acked {
            return Ok(BootstrapClaim::Unavailable);
        }
        let Some(mb) = mb else {
            // Approved, not acknowledged, no copy: only the deadline clears it
            // that way (approval writes the copy in its own transaction).
            return Ok(if cleared {
                BootstrapClaim::Expired
            } else {
                BootstrapClaim::Unavailable
            });
        };
        if past {
            // The held connection guard spans the read above and this clear,
            // so no other call can deliver the copy in between.
            expire_delivery_copies(&mut c, now, DeliveryCopies::JoinRequest(id))?;
            settle_owed_checkpoint_logged(&c);
            return Ok(BootstrapClaim::Expired);
        }
        // First-delivery timestamp, observability only — never blocks
        // re-delivery, and a failure to write it does not withhold a key that
        // was read. It rewrites the row that holds the copy, so it runs with
        // `secure_delete` like the two updates that clear it: otherwise the
        // row's previous image, key included, would stay in the page's free
        // space after the copy itself is gone.
        if !delivered {
            let stamped = with_secure_delete(&mut c, |conn| {
                conn.execute(
                    "UPDATE join_requests SET bootstrap_delivered_at = ?2 \
                     WHERE id = ?1 AND bootstrap_delivered_at IS NULL",
                    params![id, now],
                )
            });
            if let Err(error) = stamped {
                tracing::warn!(%error, "could not record a join-request key's first delivery");
            }
        }
        Ok(BootstrapClaim::Delivered(mb))
    }

    /// Finalize the bootstrap: the client has verified its mb_ (via /whoami) and
    /// persisted it, so close the re-fetch window. After this the bootstrap
    /// endpoint answers `Unavailable`. Returns `Some(true)` when this call set the ACK,
    /// `Some(false)` when the request was already acked by this same secret (a
    /// safe retry), and `None` when the request is unknown, not owned by this
    /// secret, or not in the approved state. Requiring the secret means only the
    /// requester can close their own window. A storage error is an error.
    ///
    /// An ACK after the deadline still succeeds: the requester may have
    /// collected the key in time and only lost the ACK, and it must not see a
    /// failure on every retry. Such an ACK records the receipt and nothing
    /// else: the cleared copy stays cleared and the window stays closed (the
    /// UPDATE below only ever sets `mb_token` to NULL).
    pub fn ack_join_request_bootstrap(
        &self,
        id: &str,
        secret: &str,
        now: u64,
    ) -> rusqlite::Result<Option<bool>> {
        let Some(mut c) = self.conn() else {
            return Ok(None);
        };
        // Ownership + state, reading the current ack timestamp. We deliberately do
        // NOT require `mb_token IS NOT NULL` here: a completed ACK scrubs mb_token
        // (below), so a repeat ACK by the same secret must still resolve as an
        // idempotent success, not a 404. The held connection guard spans this
        // SELECT and the UPDATE, so a concurrent bootstrap/ACK cannot interleave.
        let row: Option<(Option<u64>, bool)> = c
            .query_row(
                "SELECT bootstrap_acked_at, mb_token IS NOT NULL FROM join_requests \
                 WHERE id = ?1 AND secret_hash = ?2 AND status = 'approved'",
                params![id, secret_hash(secret)],
                |r| Ok((r.get::<_, Option<u64>>(0)?, r.get(1)?)),
            )
            .optional()?;
        let Some((acked_at, holds_copy)) = row else {
            return Ok(None);
        };
        if acked_at.is_some() {
            return Ok(Some(false)); // already finalized by this secret — idempotent
        }
        // Finalize atomically: stamp the ACK AND scrub the plaintext mb_ copy from
        // the request row. The live membership itself lives in the members and
        // credentials tables and is untouched — only this one-time-delivery copy
        // is erased. `secure_delete` zeroes the row's old image in its page;
        // the WAL frames that still hold it go with the truncating checkpoint
        // owed below (best-effort, see `settle_owed_checkpoint`).
        let changed = with_secure_delete(&mut c, |conn| {
            let tx = conn.transaction()?;
            let changed = tx.execute(
                "UPDATE join_requests SET bootstrap_acked_at = ?2, mb_token = NULL \
                 WHERE id = ?1 AND bootstrap_acked_at IS NULL",
                params![id, now],
            )?;
            if holds_copy {
                record_owed_checkpoint(&tx)?;
            }
            tx.commit()?;
            Ok(changed)
        })?;
        if holds_copy {
            settle_owed_checkpoint_logged(&c);
        }
        Ok(Some(changed == 1))
    }

    /// Clear every key delivery copy whose window has closed: a join
    /// request's (`JOIN_BOOTSTRAP_WINDOW_MS` after approval) and a membership
    /// claim's (`CLAIM_DELIVERY_WINDOW_MS` after the claim). Called by the
    /// hub's background sweep; the open and a late collection do the same.
    /// Returns how many copies were cleared. Then retries a WAL clean-up that
    /// is owed, from this sweep or an earlier ACK or expiry, best-effort.
    pub fn expire_join_request_bootstraps(&self, now: u64) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
        let cleared = expire_delivery_copies(&mut c, now, DeliveryCopies::PastDeadline)?;
        settle_owed_checkpoint_logged(&c);
        Ok(cleared)
    }

    /// The first use of an invite (the first session or loca socket taken with
    /// it): the holder has proved it has the invite, so its delivery copy
    /// ends and `first_used_at` is stamped, in one write with
    /// `secure_delete` on. By the invite's hash. Returns whether this call
    /// cleared a copy. The WAL clean-up it leaves is owed and paid
    /// best-effort (as for join requests).
    pub fn end_invite_delivery_by_hash(
        &self,
        invite_hash: &str,
        now: u64,
    ) -> rusqlite::Result<bool> {
        let Some(mut c) = self.conn() else {
            return Ok(false);
        };
        let cleared = with_secure_delete(&mut c, |conn| {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let cleared = tx.execute(
                "UPDATE invites SET delivery_token = NULL, first_used_at = ?2 \
                 WHERE secret_hash = ?1 AND delivery_token IS NOT NULL",
                params![invite_hash, now],
            )?;
            if cleared > 0 {
                record_owed_checkpoint(&tx)?;
            }
            tx.commit()?;
            Ok(cleared > 0)
        })?;
        if cleared {
            settle_owed_checkpoint_logged(&c);
        }
        Ok(cleared)
    }

    /// Clear the invite delivery copies whose window has closed at `now`
    /// (`INVITE_DELIVERY_WINDOW_MS` after issue) or whose invite has ended
    /// (only the invite whose hash is `only_hash`, when given: a delivery
    /// attempted too late). Called by the hub's background sweep and by a
    /// Lobby connect that finds a copy past its deadline; the open does the
    /// same. Then retries an owed WAL clean-up, best-effort. Returns how many
    /// copies were cleared.
    pub fn expire_invite_deliveries(
        &self,
        now: u64,
        only_hash: Option<&str>,
    ) -> rusqlite::Result<usize> {
        let Some(mut c) = self.conn() else {
            return Ok(0);
        };
        let cleared = expire_invite_copies(&mut c, now, only_hash)?;
        settle_owed_checkpoint_logged(&c);
        Ok(cleared)
    }

    /// The live invites whose delivery copy is still held, by that copy. Read
    /// once at start; an unreadable table stops the start.
    pub fn load_invite_deliveries(&self) -> rusqlite::Result<Vec<String>> {
        let Some(c) = self.conn() else {
            return Ok(Vec::new());
        };
        let mut statement = c.prepare(
            "SELECT delivery_token FROM invites \
             WHERE revoked_at IS NULL AND delivery_token IS NOT NULL",
        )?;
        let rows = statement.query_map([], |r| r.get(0))?;
        rows.collect()
    }

    /// Test-only: an invite's raw `(delivery_token, first_used_at)`, by the
    /// invite's hash, or `None` when the row is absent.
    #[cfg(test)]
    pub fn invite_delivery_raw_by_hash(
        &self,
        invite_hash: &str,
    ) -> Option<(Option<String>, Option<u64>)> {
        let c = self.conn()?;
        c.query_row(
            "SELECT delivery_token, first_used_at FROM invites WHERE secret_hash = ?1",
            params![invite_hash],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()
    }

    /// Test-only: whether a truncating WAL checkpoint is still owed after a
    /// key copy was cleared: `Ok(true)` while the WAL may still hold an old copy.
    #[cfg(test)]
    pub fn join_key_checkpoint_owed(&self) -> rusqlite::Result<bool> {
        let Some(c) = self.conn() else {
            return Ok(false);
        };
        checkpoint_owed(&c)
    }

    /// Test-only: read the raw stored `mb_token` for a request. `Some(Some(_))`
    /// when a credential copy is present, `Some(None)` when it has been scrubbed,
    /// `None` when the row is absent. Proves the ACK erases the credential.
    #[cfg(test)]
    pub fn join_request_mb_token_raw(&self, id: &str) -> Option<Option<String>> {
        let c = self.conn()?;
        c.query_row(
            "SELECT mb_token FROM join_requests WHERE id = ?1",
            params![id],
            |r| r.get::<_, Option<String>>(0),
        )
        .ok()
    }
}
