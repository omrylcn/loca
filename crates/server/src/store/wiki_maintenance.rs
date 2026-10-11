//! Durable bounded Wiki maintenance, separate from chat and model turns.
use super::*;
use serde::{Deserialize, Serialize};

const MAX_BATCH_MESSAGES: usize = 100;
const MAX_PENDING_AGE_MS: u64 = 15 * 60 * 1000;
const LEASE_MS: u64 = 5 * 60 * 1000;
const MAX_WORKER_MS: u64 = 30 * 60 * 1000;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WikiLease {
    pub batch_id: u64,
    pub epoch: u64,
}

#[derive(Debug, Serialize)]
pub struct WikiBatch {
    pub lease: WikiLease,
    pub room: String,
    pub after_id: u64,
    pub through_id: u64,
    pub reason: String,
    pub lease_expires_at: u64,
    pub attempts: u64,
    pub messages: Vec<Message>,
}

fn failure(_: rusqlite::Error) -> WikiError {
    WikiError::Storage
}

pub(super) fn lease_valid(
    c: &Connection,
    room: &str,
    principal: &str,
    lease: &WikiLease,
    at: u64,
) -> Result<bool, WikiError> {
    c.query_row(
        "SELECT EXISTS(SELECT 1 FROM loca_wiki_batches b JOIN loca_wiki w ON w.room=b.room
         WHERE b.id=?1 AND b.epoch=?2 AND b.room=?3 AND b.editor_principal=?4
         AND b.status='running' AND b.lease_expires_at>?5
         AND w.enabled=1 AND w.editor_principal=b.editor_principal)",
        params![lease.batch_id, lease.epoch, room, principal, at],
        |r| r.get(0),
    )
    .map_err(failure)
}

impl Store {
    pub fn cancel_wiki_review(&self, room: &str) -> Result<(), WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        c.execute(
            "UPDATE loca_wiki_batches SET status='cancelled' WHERE room=?1
                  AND status IN ('queued','running','blocked')",
            params![room],
        )
        .map_err(failure)?;
        Ok(())
    }

    pub fn wiki_maintenance_rooms(&self) -> Result<Vec<String>, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let mut stmt = c
            .prepare("SELECT room FROM loca_wiki WHERE enabled=1")
            .map_err(failure)?;
        let rows = stmt.query_map([], |r| r.get(0)).map_err(failure)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(failure)
    }

    pub fn wiki_batch_status(&self, room: &str) -> Result<Option<String>, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        c.query_row(
            "SELECT status FROM loca_wiki_batches WHERE room=?1 ORDER BY id DESC LIMIT 1",
            params![room],
            |r| r.get(0),
        )
        .optional()
        .map_err(failure)
    }

    /// Scheduler has no model work and no chat side effects. Only a bounded
    /// source window is read; pending batches are frozen for cursor coverage.
    pub fn schedule_wiki_review(&self, room: &str, at: u64) -> Result<bool, WikiError> {
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(failure)?;
        let config: Option<(String, u64, u64)> = tx.query_row(
            "SELECT editor_principal, reviewed_through, interval_messages FROM loca_wiki WHERE room=?1 AND enabled=1",
            params![room], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
        ).optional().map_err(failure)?;
        let Some((editor, cursor, interval)) = config else {
            return Ok(false);
        };
        let active: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM loca_wiki_batches WHERE room=?1 AND status IN ('queued','running','blocked'))",
            params![room], |r| r.get(0),
        ).map_err(failure)?;
        if active {
            return Ok(false);
        }
        let inputs = {
            let mut stmt = tx
                .prepare(
                    "SELECT id,ts FROM messages WHERE room=?1 AND id>?2 AND kind='say'
                 AND sender_type IN ('user','agent') AND sender!='loca' ORDER BY id LIMIT 1000",
                )
                .map_err(failure)?;
            let rows = stmt
                .query_map(params![room, cursor], |r| {
                    Ok((r.get::<_, u64>(0)?, r.get::<_, u64>(1)?))
                })
                .map_err(failure)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(failure)?
        };
        let Some(oldest_ts) = inputs.iter().map(|(_, ts)| *ts).min() else {
            return Ok(false);
        };
        let reason = if inputs.len() as u64 >= interval {
            "message_threshold"
        } else if at.saturating_sub(oldest_ts) >= MAX_PENDING_AGE_MS {
            "source_age"
        } else {
            return Ok(false);
        };
        let through = inputs[inputs.len().min(MAX_BATCH_MESSAGES) - 1].0;
        tx.execute("INSERT INTO loca_wiki_batches(room,editor_principal,after_id,through_id,reason,created_at)
                    VALUES(?1,?2,?3,?4,?5,?6)",
            params![room,editor,cursor,through,reason,at]).map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn valid_wiki_lease(
        &self,
        room: &str,
        principal: &str,
        lease: &WikiLease,
        at: u64,
    ) -> Result<bool, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        lease_valid(&c, room, principal, lease, at)
    }

    pub fn claim_wiki_review(
        &self,
        room: &str,
        principal: &str,
        at: u64,
    ) -> Result<Option<WikiBatch>, WikiError> {
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(failure)?;
        let (editor, enabled): (String, bool) = tx
            .query_row(
                "SELECT editor_principal,enabled FROM loca_wiki WHERE room=?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(failure)?
            .ok_or(WikiError::NotConfigured)?;
        if editor != principal {
            return Err(WikiError::NotEditor);
        }
        if !enabled {
            return Ok(None);
        }
        tx.execute(
            "UPDATE loca_wiki_batches SET status='blocked' WHERE room=?1 AND status='running'
                    AND lease_expires_at<=?2 AND attempts>=3",
            params![room, at],
        )
        .map_err(failure)?;
        let job: Option<(u64,u64,u64,String,u64,u64)> = tx.query_row(
            "SELECT id,after_id,through_id,reason,epoch,attempts FROM loca_wiki_batches WHERE room=?1
             AND editor_principal=?2 AND (status='queued' OR (status='running' AND lease_expires_at<=?3)) LIMIT 1",
            params![room,principal,at], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)),
        ).optional().map_err(failure)?;
        let Some((id, after, through, reason, epoch, attempts)) = job else {
            tx.commit().map_err(failure)?;
            return Ok(None);
        };
        let expires = at.checked_add(LEASE_MS).ok_or(WikiError::InvalidInput)?;
        let next_epoch = epoch.checked_add(1).ok_or(WikiError::Storage)?;
        tx.execute("UPDATE loca_wiki_batches SET status='running',epoch=?2,attempts=attempts+1,lease_expires_at=?3 WHERE id=?1",
            params![id,next_epoch,expires]).map_err(failure)?;
        tx.execute(
            "INSERT INTO loca_wiki_leases(batch_id,epoch,started_at) VALUES(?1,?2,?3)",
            params![id, next_epoch, at],
        )
        .map_err(failure)?;
        let messages = {
            let mut stmt = tx.prepare(
                "SELECT id,sender,sender_type,target,text,reply_to,ts,kind,attachments FROM messages
                 WHERE room=?1 AND id>?2 AND id<=?3 AND kind='say'
                 AND sender_type IN ('user','agent') AND sender!='loca' ORDER BY id LIMIT 100",
            ).map_err(failure)?;
            let rows = stmt
                .query_map(params![room, after, through], |r| {
                    Ok(Message {
                        id: r.get(0)?,
                        room: room.into(),
                        sender: r.get(1)?,
                        sender_type: parse_sender_type(&r.get::<_, String>(2)?),
                        target: r.get(3)?,
                        text: r.get(4)?,
                        reply_to: r.get(5)?,
                        reply_to_sender: None,
                        ts: r.get(6)?,
                        kind: parse_kind(&r.get::<_, String>(7)?),
                        attachments: attachments_from_json(r.get(8)?),
                    })
                })
                .map_err(failure)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(failure)?
        };
        tx.commit().map_err(failure)?;
        Ok(Some(WikiBatch {
            lease: WikiLease {
                batch_id: id,
                epoch: next_epoch,
            },
            room: room.into(),
            after_id: after,
            through_id: through,
            reason,
            lease_expires_at: expires,
            attempts: attempts + 1,
            messages,
        }))
    }

    /// Renew only the current epoch, never revive an expired worker. The
    /// absolute lifetime is bounded even if a stuck worker keeps heartbeating.
    pub fn renew_wiki_review(
        &self,
        room: &str,
        principal: &str,
        lease: &WikiLease,
        at: u64,
    ) -> Result<u64, WikiError> {
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(failure)?;
        if !lease_valid(&tx, room, principal, lease, at)? {
            return Err(WikiError::Conflict);
        }
        let (started, existing): (u64, u64) = tx
            .query_row(
                "SELECT l.started_at,b.lease_expires_at FROM loca_wiki_leases l
             JOIN loca_wiki_batches b ON b.id=l.batch_id
             WHERE l.batch_id=?1 AND l.epoch=?2",
                params![lease.batch_id, lease.epoch],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(failure)?;
        let deadline = started
            .checked_add(MAX_WORKER_MS)
            .ok_or(WikiError::InvalidInput)?;
        let expires = existing
            .max(at.checked_add(LEASE_MS).ok_or(WikiError::InvalidInput)?)
            .min(deadline);
        if expires <= at {
            return Err(WikiError::Conflict);
        }
        tx.execute(
            "UPDATE loca_wiki_batches SET lease_expires_at=?2 WHERE id=?1",
            params![lease.batch_id, expires],
        )
        .map_err(failure)?;
        tx.commit().map_err(failure)?;
        Ok(expires)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::wiki::WikiPage;

    fn setup() -> Store {
        let s = Store::open(Some(":memory:")).unwrap();
        s.configure_wiki("a", "editor", true, 3).unwrap();
        s
    }

    fn message(s: &Store, id: u64, room: &str, ts: u64) {
        s.conn().unwrap().execute(
            "INSERT INTO messages(id,room,sender,sender_type,text,ts,kind) VALUES(?1,?2,'user','user','source',?3,'say')",
            params![id,room,ts],
        ).unwrap();
    }

    #[test]
    fn wiki_batch_threshold_scope_and_coalescing() {
        let s = setup();
        message(&s, 1, "a", 1);
        message(&s, 2, "other", 1);
        message(&s, 3, "a", 2);
        assert!(!s.schedule_wiki_review("a", 10).unwrap());
        message(&s, 4, "a", 3);
        assert!(s.schedule_wiki_review("a", 10).unwrap());
        message(&s, 5, "a", 4);
        assert!(!s.schedule_wiki_review("a", 11).unwrap());
        let batch = s.claim_wiki_review("a", "editor", 11).unwrap().unwrap();
        assert_eq!(
            batch.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![1, 3, 4]
        );
        assert_eq!(batch.through_id, 4);
        assert!(s.claim_wiki_review("a", "editor", 12).unwrap().is_none());
    }

    #[test]
    fn wiki_batch_age_is_oldest_source_not_last_activity() {
        let s = setup();
        assert!(!s.schedule_wiki_review("a", MAX_PENDING_AGE_MS * 2).unwrap());
        message(&s, 1, "a", 1000);
        message(&s, 2, "a", MAX_PENDING_AGE_MS);
        assert!(s
            .schedule_wiki_review("a", MAX_PENDING_AGE_MS + 1000)
            .unwrap());
    }

    #[test]
    fn wiki_batch_age_handles_clock_rollback_without_reordering_sources() {
        let s = setup();
        message(&s, 1, "a", MAX_PENDING_AGE_MS * 3);
        message(&s, 2, "a", 100);
        assert!(s
            .schedule_wiki_review("a", MAX_PENDING_AGE_MS + 100)
            .unwrap());
        let batch = s
            .claim_wiki_review("a", "editor", MAX_PENDING_AGE_MS + 101)
            .unwrap()
            .unwrap();
        assert_eq!(
            batch.messages.iter().map(|m| m.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(batch.reason, "source_age");
    }

    #[test]
    fn wiki_batch_role_disable_and_reassignment_fence() {
        let s = setup();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", MAX_PENDING_AGE_MS + 1).unwrap();
        assert!(matches!(
            s.claim_wiki_review("a", "reader", 2),
            Err(WikiError::NotEditor)
        ));
        let old = s.claim_wiki_review("a", "editor", 2).unwrap().unwrap();
        s.configure_wiki("a", "replacement", true, 3).unwrap();
        assert!(!s.valid_wiki_lease("a", "editor", &old.lease, 3).unwrap());
        assert!(s.schedule_wiki_review("a", MAX_PENDING_AGE_MS + 2).unwrap());
        let new = s.claim_wiki_review("a", "replacement", 3).unwrap().unwrap();
        s.configure_wiki("a", "replacement", false, 3).unwrap();
        assert!(!s
            .valid_wiki_lease("a", "replacement", &new.lease, 4)
            .unwrap());
        assert!(s
            .claim_wiki_review("a", "replacement", 4)
            .unwrap()
            .is_none());
    }

    #[test]
    fn wiki_room_closure_cancels_work_without_advancing_review_cursor() {
        let s = setup();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", MAX_PENDING_AGE_MS + 1).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 2).unwrap().unwrap();
        s.cancel_wiki_review("a").unwrap();
        assert!(!s.valid_wiki_lease("a", "editor", &batch.lease, 3).unwrap());
        assert_eq!(
            s.wiki_batch_status("a").unwrap().as_deref(),
            Some("cancelled")
        );
        assert_eq!(s.wiki_snapshot("a").unwrap().reviewed_through, 0);
        assert!(s.claim_wiki_review("a", "editor", 3).unwrap().is_none());
        assert_eq!(s.messages_after("a", 0, 10).unwrap().unwrap().len(), 1);
    }

    #[test]
    fn wiki_batch_expired_claim_is_fenced_and_retry_is_bounded() {
        let s = setup();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", MAX_PENDING_AGE_MS + 1).unwrap();
        let first = s.claim_wiki_review("a", "editor", 2).unwrap().unwrap();
        let second = s
            .claim_wiki_review("a", "editor", LEASE_MS + 3)
            .unwrap()
            .unwrap();
        assert!(second.lease.epoch > first.lease.epoch);
        assert!(!s
            .valid_wiki_lease("a", "editor", &first.lease, LEASE_MS + 4)
            .unwrap());
        assert!(s
            .claim_wiki_review("a", "editor", LEASE_MS * 2 + 4)
            .unwrap()
            .is_some());
        assert!(s
            .claim_wiki_review("a", "editor", LEASE_MS * 3 + 5)
            .unwrap()
            .is_none());
        assert_eq!(
            s.wiki_batch_status("a").unwrap().as_deref(),
            Some("blocked")
        );
    }

    #[test]
    fn wiki_batch_finish_is_atomic_and_never_inherits_unreviewed_messages() {
        let s = setup();
        for id in 1..=3 {
            message(&s, id, "a", id);
        }
        s.schedule_wiki_review("a", 10).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 11).unwrap().unwrap();
        message(&s, 4, "a", 12);
        let mut review = WikiCommit {
            expected_revision: 0,
            reviewed_through: 4,
            reason: "worker reviewed bounded sources".into(),
            pages: vec![],
        };
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 13),
            Err(WikiError::Conflict)
        );
        assert_eq!(s.wiki_snapshot("a").unwrap().reviewed_through, 0);
        assert_eq!(
            s.wiki_batch_status("a").unwrap().as_deref(),
            Some("running")
        );
        review.reviewed_through = 3;
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 14),
            Ok(0)
        );
        assert_eq!(s.wiki_snapshot("a").unwrap().reviewed_through, 3);
        assert_eq!(
            s.wiki_batch_status("a").unwrap().as_deref(),
            Some("completed")
        );
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 15),
            Ok(0)
        );
        assert!(s
            .schedule_wiki_review("a", MAX_PENDING_AGE_MS + 12)
            .unwrap());
        assert_eq!(
            s.claim_wiki_review("a", "editor", MAX_PENDING_AGE_MS + 13)
                .unwrap()
                .unwrap()
                .messages[0]
                .id,
            4
        );
    }

    #[test]
    fn wiki_batch_bounds_backlog_and_excludes_system_announcements() {
        let s = setup();
        for id in 1..=150 {
            message(&s, id, "a", id);
        }
        s.conn()
            .unwrap()
            .execute("UPDATE messages SET kind='announce' WHERE id=1", [])
            .unwrap();
        s.conn()
            .unwrap()
            .execute("UPDATE messages SET sender='loca' WHERE id=2", [])
            .unwrap();
        s.schedule_wiki_review("a", 200).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 201).unwrap().unwrap();
        assert_eq!(batch.messages.len(), 100);
        assert_eq!(batch.messages.first().unwrap().id, 3);
        assert_eq!(batch.messages.last().unwrap().id, 102);
    }

    #[test]
    fn wiki_batch_manual_review_cancels_stale_work_without_erasing_sources() {
        let s = setup();
        for id in 1..=3 {
            message(&s, id, "a", id);
        }
        s.schedule_wiki_review("a", 10).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 11).unwrap().unwrap();
        let review = WikiCommit {
            expected_revision: 0,
            reviewed_through: 3,
            reason: "manual review".into(),
            pages: vec![],
        };
        s.commit_wiki("a", "editor", &review, 12).unwrap();
        assert!(!s.valid_wiki_lease("a", "editor", &batch.lease, 13).unwrap());
        assert_eq!(s.messages_after("a", 0, 10).unwrap().unwrap().len(), 3);
        assert!(!s.schedule_wiki_review("a", MAX_PENDING_AGE_MS * 2).unwrap());
    }

    #[test]
    fn wiki_batch_and_lease_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("wiki.db");
        let path = db.to_str().unwrap();
        let s = Store::open(Some(path)).unwrap();
        s.configure_wiki("a", "editor", true, 1).unwrap();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", 2).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 3).unwrap().unwrap();
        drop(s);
        let restored = Store::open(Some(path)).unwrap();
        assert!(!restored.schedule_wiki_review("a", 4).unwrap());
        assert!(restored
            .claim_wiki_review("a", "editor", 4)
            .unwrap()
            .is_none());
        assert!(restored
            .valid_wiki_lease("a", "editor", &batch.lease, 4)
            .unwrap());
        assert_eq!(restored.wiki_snapshot("a").unwrap().reviewed_through, 0);
    }

    #[test]
    fn wiki_finish_retry_survives_restart_and_rejects_changed_payload() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("wiki.db");
        let path = db.to_str().unwrap();
        let s = Store::open(Some(path)).unwrap();
        s.configure_wiki("a", "editor", true, 1).unwrap();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", 2).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 3).unwrap().unwrap();
        let mut review = WikiCommit {
            expected_revision: 0,
            reviewed_through: 1,
            reason: "isolated worker".into(),
            pages: vec![WikiPage {
                slug: "overview".into(),
                title: "Overview".into(),
                body: "Accepted result".into(),
                sources: vec![1],
                revision: 0,
            }],
        };
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 4),
            Ok(1)
        );
        drop(s);
        let s = Store::open(Some(path)).unwrap();
        // Even expiry and later publication do not turn a lost response into a new edit.
        s.commit_wiki(
            "a",
            "editor",
            &WikiCommit {
                expected_revision: 1,
                reviewed_through: 1,
                reason: "later accepted edit".into(),
                pages: vec![WikiPage {
                    slug: "overview".into(),
                    title: "Overview".into(),
                    body: "Newer result".into(),
                    sources: vec![1],
                    revision: 0,
                }],
            },
            5,
        )
        .unwrap();
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, LEASE_MS * 2),
            Ok(1)
        );
        assert_eq!(s.wiki_snapshot("a").unwrap().revision, 2);
        review.reason = "changed retry".into();
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 6),
            Err(WikiError::Conflict)
        );
        review.reason = "isolated worker".into();
        s.configure_wiki("a", "replacement", true, 1).unwrap();
        assert_eq!(
            s.finish_wiki_review("a", "editor", &review, &batch.lease, 7),
            Err(WikiError::NotEditor)
        );
        assert_eq!(
            s.finish_wiki_review("a", "replacement", &review, &batch.lease, 7),
            Err(WikiError::Conflict)
        );
    }

    #[test]
    fn wiki_renewal_is_fenced_and_has_absolute_limit() {
        let s = setup();
        message(&s, 1, "a", 1);
        s.schedule_wiki_review("a", MAX_PENDING_AGE_MS + 1).unwrap();
        let batch = s.claim_wiki_review("a", "editor", 2).unwrap().unwrap();
        assert_eq!(
            s.renew_wiki_review("a", "reader", &batch.lease, 3),
            Err(WikiError::Conflict)
        );
        assert_eq!(
            s.renew_wiki_review("a", "editor", &batch.lease, 1),
            Ok(LEASE_MS + 2)
        );
        for tick in 1..=5 {
            assert_eq!(
                s.renew_wiki_review("a", "editor", &batch.lease, tick * LEASE_MS - tick),
                Ok((tick + 1) * LEASE_MS - tick)
            );
        }
        assert_eq!(
            s.renew_wiki_review("a", "editor", &batch.lease, MAX_WORKER_MS - 6),
            Ok(MAX_WORKER_MS + 2)
        );
        assert_eq!(
            s.renew_wiki_review("a", "editor", &batch.lease, MAX_WORKER_MS + 2),
            Err(WikiError::Conflict)
        );
        let new = s
            .claim_wiki_review("a", "editor", MAX_WORKER_MS + 3)
            .unwrap()
            .unwrap();
        assert!(new.lease.epoch > batch.lease.epoch);
        assert_eq!(
            s.renew_wiki_review("a", "editor", &batch.lease, MAX_WORKER_MS + 4),
            Err(WikiError::Conflict)
        );
        s.configure_wiki("a", "editor", false, 3).unwrap();
        assert_eq!(
            s.renew_wiki_review("a", "editor", &new.lease, MAX_WORKER_MS + 5),
            Err(WikiError::Conflict)
        );
    }
}
