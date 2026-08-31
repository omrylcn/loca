//! Attachment lifecycle over SQLite: quotas, the pending→referenced flip, the
//! two refcount levels, and the orphan/GC sweep. The physical bytes live in the
//! content-addressed [`BlobStore`](super::attachments::BlobStore); this file is
//! the metadata + reference index that decides what may be served and when a
//! file may be deleted.
//!
//! Design note — NO stored refcount. Both levels loca-dev requires are derived
//! straight from `attachment_refs`, so nothing can drift out of sync:
//!   * per-room LOGICAL size / GET-auth = rows `WHERE room = ?`;
//!   * GLOBAL PHYSICAL liveness         = `COUNT(*) WHERE sha = ?` across ALL
//!     rooms — the file is deleted only when that reaches 0, so deleting one
//!     room can never corrupt a blob another room still shares.
use super::*;
use sha2::{Digest, Sha256};

/// Why an attachment upload was refused. The route maps each to an HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachError {
    /// Memory-only mode has no durable blob dir — attachments are off (503).
    Disabled,
    /// Adding this file would exceed the per-room logical quota (413).
    QuotaRoom,
    /// Adding this file would exceed the building physical quota (413).
    QuotaBuilding,
    /// A storage (io/db) failure — not the caller's fault (503).
    Storage,
}

/// Serve-time metadata for one blob.
pub(crate) struct BlobServe {
    pub bytes: Vec<u8>,
    pub mime: String,
    pub name: String,
}

fn sha_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

impl Store {
    /// Attachments require a durable blob dir AND a database. Memory-only
    /// stores have neither, so the endpoints answer 503 there.
    pub(crate) fn attachments_enabled(&self) -> bool {
        self.blobs.is_some() && self.conn.is_some()
    }

    /// Store freshly-uploaded bytes as a `pending` blob in `room`. `mime` is
    /// the server-SNIFFED type (never the client's claim); `name` is already
    /// sanitized display metadata. Idempotent by content: identical bytes
    /// dedupe to one physical file and never double-count a quota. Returns the
    /// ref object handed back to the uploader (`id == sha256`).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn put_pending_attachment(
        &self,
        room: &str,
        uploader: &str,
        name: &str,
        mime: &str,
        bytes: &[u8],
        now: u64,
        room_max: u64,
        building_max: u64,
    ) -> Result<protocol::Attachment, AttachError> {
        let Some(blobs) = self.blobs.as_ref() else {
            return Err(AttachError::Disabled);
        };
        let Some(c) = self.conn() else {
            return Err(AttachError::Disabled);
        };
        let sha = sha_hex(bytes);
        let size = bytes.len() as u64;

        // Quota is checked against the *marginal* cost: a blob that already
        // exists physically adds nothing to the building total, and one already
        // present in this room (referenced or pending) adds nothing to the room
        // total. So re-uploading the same file is free — dedup, not double-count.
        let blob_is_new = !row_exists(&c, "SELECT 1 FROM attachment_blobs WHERE sha = ?1", &sha)
            .map_err(|_| AttachError::Storage)?;
        let new_to_room = !room_has_sha(&c, room, &sha).map_err(|_| AttachError::Storage)?;

        if blob_is_new {
            let building_used =
                sum_i64(&c, "SELECT COALESCE(SUM(size),0) FROM attachment_blobs", [])
                    .map_err(|_| AttachError::Storage)?;
            if building_used.saturating_add(size) > building_max {
                return Err(AttachError::QuotaBuilding);
            }
        }
        if new_to_room {
            let room_used = sum_i64(
                &c,
                "SELECT COALESCE(SUM(size),0) FROM (
                    SELECT DISTINCT sha, size FROM attachment_refs WHERE room = ?1
                    UNION
                    SELECT sha, size FROM attachment_pending WHERE room = ?1
                 )",
                params![room],
            )
            .map_err(|_| AttachError::Storage)?;
            if room_used.saturating_add(size) > room_max {
                return Err(AttachError::QuotaRoom);
            }
        }

        // Write the file first (atomic + dedup inside the BlobStore), then
        // record the metadata. If the DB write failed after the file landed,
        // the sweep collects the now-orphan blob (no pending, no ref) later.
        blobs.put(bytes).map_err(|_| AttachError::Storage)?;
        c.execute(
            "INSERT OR IGNORE INTO attachment_blobs (sha, mime, size, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![sha, mime, size as i64, now as i64],
        )
        .map_err(|_| AttachError::Storage)?;
        c.execute(
            "INSERT OR REPLACE INTO attachment_pending
             (sha, room, uploader, name, mime, size, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![sha, room, uploader, name, mime, size as i64, now as i64],
        )
        .map_err(|_| AttachError::Storage)?;

        Ok(protocol::Attachment {
            id: sha.clone(),
            sha256: sha,
            name: name.to_string(),
            mime: mime.to_string(),
            size,
        })
    }

    /// Resolve an attachment id the sender cited into its full ref object,
    /// using the metadata recorded at upload. Available means: pending in this
    /// room (the normal upload→send), or already referenced in this room (a
    /// re-send of the same file). An id with no such record in this room is
    /// `None` — the caller rejects the post, so a message can never cite a blob
    /// it isn't allowed to, and no cross-room hash guessing works.
    pub(crate) fn resolve_room_attachment(
        &self,
        room: &str,
        id: &str,
    ) -> Option<protocol::Attachment> {
        let c = self.conn()?;
        // Prefer the pending upload; fall back to an existing room reference.
        let row = c
            .query_row(
                "SELECT name, mime, size FROM attachment_pending WHERE sha = ?1 AND room = ?2",
                params![id, room],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .ok()?
            .or_else(|| {
                c.query_row(
                    "SELECT name, mime, size FROM attachment_refs
                     WHERE sha = ?1 AND room = ?2 LIMIT 1",
                    params![id, room],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, i64>(2)?,
                        ))
                    },
                )
                .optional()
                .ok()
                .flatten()
            })?;
        Some(protocol::Attachment {
            id: id.to_string(),
            sha256: id.to_string(),
            name: row.0,
            mime: row.1,
            size: row.2 as u64,
        })
    }

    /// Flip the cited blobs `pending → referenced` for one accepted message, in
    /// a single transaction: insert the per-room ref rows and clear the pending
    /// slots together. The blob is protected from the sweep at every instant —
    /// by its pending row before this commit, by its ref row after — so a
    /// concurrent sweep/delete can never collect a blob a live message cites.
    pub(crate) fn reference_attachments(
        &self,
        room: &str,
        message_id: u64,
        attachments: &[protocol::Attachment],
        now: u64,
    ) -> rusqlite::Result<()> {
        if attachments.is_empty() {
            return Ok(());
        }
        let Some(mut c) = self.conn() else {
            return Ok(());
        };
        let tx = c.transaction()?;
        for a in attachments {
            // The blob row exists from upload; keep this defensive so a
            // reference always has its serve-metadata anchor.
            tx.execute(
                "INSERT OR IGNORE INTO attachment_blobs (sha, mime, size, created_at)
                 VALUES (?1, ?2, ?3, ?4)",
                params![a.sha256, a.mime, a.size as i64, now as i64],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO attachment_refs
                 (room, message_id, sha, name, mime, size, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    room,
                    message_id as i64,
                    a.sha256,
                    a.name,
                    a.mime,
                    a.size as i64,
                    now as i64
                ],
            )?;
            tx.execute(
                "DELETE FROM attachment_pending WHERE sha = ?1 AND room = ?2",
                params![a.sha256, room],
            )?;
        }
        tx.commit()
            .inspect_err(|e| tracing::error!(error = %e, "reference_attachments failed"))
    }

    /// Read a blob for serving IF it is referenced by a message in THIS room.
    /// The room gate is the whole authorization: an id not cited in `room` is
    /// `None` (404) even when the bytes exist for another room — no cross-room
    /// read with a guessed hash. Verifies the file still hashes to its id
    /// (BlobStore::read), so a corrupted file is a 404, never wrong bytes.
    pub(crate) fn read_room_attachment(&self, room: &str, id: &str) -> Option<BlobServe> {
        let blobs = self.blobs.as_ref()?;
        let c = self.conn()?;
        let (name, mime) = c
            .query_row(
                "SELECT name, mime FROM attachment_refs WHERE room = ?1 AND sha = ?2 LIMIT 1",
                params![room, id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()
            .ok()??;
        drop(c);
        let bytes = blobs.read(id).ok()?;
        Some(BlobServe { bytes, mime, name })
    }

    /// Collect orphans and GC'd blobs. Two waves, idempotent. First, drop
    /// `pending` uploads older than `ttl_ms` (never sent). Then delete the file
    /// and blob row for any blob with neither a ref nor a pending slot — an
    /// upload swept in the first wave, or a blob whose last referencing message
    /// was deleted (CASCADE dropped its refs). Returns how many physical files
    /// were deleted (for telemetry/tests).
    pub(crate) fn sweep_attachments(&self, now: u64, ttl_ms: u64) -> usize {
        let Some(blobs) = self.blobs.as_ref() else {
            return 0;
        };
        let Some(c) = self.conn() else {
            return 0;
        };
        let cutoff = (now as i64).saturating_sub(ttl_ms as i64);
        let _ = c.execute(
            "DELETE FROM attachment_pending WHERE created_at < ?1",
            params![cutoff],
        );
        let orphans: Vec<String> = {
            let mut stmt = match c.prepare(
                "SELECT sha FROM attachment_blobs b
                 WHERE NOT EXISTS (SELECT 1 FROM attachment_refs r WHERE r.sha = b.sha)
                   AND NOT EXISTS (SELECT 1 FROM attachment_pending p WHERE p.sha = b.sha)",
            ) {
                Ok(s) => s,
                Err(_) => return 0,
            };
            let rows = match stmt.query_map([], |r| r.get::<_, String>(0)) {
                Ok(rows) => rows,
                Err(_) => return 0,
            };
            rows.filter_map(|r| r.ok()).collect()
        };
        let mut deleted = 0;
        for sha in orphans {
            // Delete the file first; only drop the DB row if the file is gone,
            // so a delete failure retries next sweep instead of leaking the row.
            if blobs.delete(&sha).is_ok() {
                let _ = c.execute("DELETE FROM attachment_blobs WHERE sha = ?1", params![sha]);
                deleted += 1;
            }
        }
        deleted
    }
}

fn row_exists(c: &Connection, sql: &str, param: &str) -> rusqlite::Result<bool> {
    Ok(c.query_row(sql, params![param], |_| Ok(()))
        .optional()?
        .is_some())
}

fn room_has_sha(c: &Connection, room: &str, sha: &str) -> rusqlite::Result<bool> {
    Ok(c.query_row(
        "SELECT 1 FROM attachment_refs WHERE room = ?1 AND sha = ?2
         UNION SELECT 1 FROM attachment_pending WHERE room = ?1 AND sha = ?2 LIMIT 1",
        params![room, sha],
        |_| Ok(()),
    )
    .optional()?
    .is_some())
}

fn sum_i64<P: rusqlite::Params>(c: &Connection, sql: &str, params: P) -> rusqlite::Result<u64> {
    let v: i64 = c.query_row(sql, params, |r| r.get(0))?;
    Ok(v.max(0) as u64)
}
