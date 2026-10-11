//! Durable page selection and native acceptance, never a chat/LLM trigger.
use super::*;
use serde::Deserialize;

#[cfg(test)]
#[path = "tests/wiki_context.rs"]
mod tests;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WikiContextSelection {
    pub slugs: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WikiContextAck {
    pub context_id: String,
    pub packet_id: String,
    pub revision: u64,
    pub selection_version: u64,
}

fn failure(_: rusqlite::Error) -> WikiError {
    WikiError::Storage
}

impl Store {
    pub fn wiki_context_requests(&self, room: &str) -> Result<serde_json::Value, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let mut stmt = c.prepare("SELECT p.display_name,s.slugs,s.reason,s.version FROM wiki_context_selection s
            JOIN principals p ON p.id=s.principal WHERE s.room=?1 AND p.kind='agent' AND p.disabled_at IS NULL
            ORDER BY s.requested_at DESC LIMIT 128").map_err(failure)?;
        let rows = stmt
            .query_map(params![room], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, u64>(3)?,
                ))
            })
            .map_err(failure)?;
        let mut requests = Vec::new();
        for row in rows {
            let (target, slugs, reason, version) = row.map_err(failure)?;
            requests.push(serde_json::json!({"target":target,"slugs":serde_json::from_str::<Vec<String>>(&slugs).map_err(|_|WikiError::Storage)?,"reason":reason,"version":version}));
        }
        Ok(serde_json::json!({"requests":requests}))
    }
    pub fn wiki_native_ready(
        &self,
        room: &str,
        principal: &str,
        runtime: &str,
        at: u64,
    ) -> Result<(), WikiError> {
        if !matches!(runtime, "codex" | "claude") {
            return Err(WikiError::InvalidInput);
        }
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        c.execute("INSERT INTO wiki_native_runtime VALUES(?1,?2,?3,?4)
            ON CONFLICT(room,principal) DO UPDATE SET runtime=excluded.runtime,ready_at=excluded.ready_at",
            params![room,principal,runtime,at]).map_err(failure)?;
        Ok(())
    }

    pub fn wiki_editor_runtime_ready(
        &self,
        room: &str,
        principal: &str,
        at: u64,
    ) -> Result<bool, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM wiki_native_runtime WHERE room=?1 AND principal=?2
            AND ready_at<=?3 AND ready_at>=?4)",
            params![room, principal, at, at.saturating_sub(120_000)],
            |r| r.get(0),
        )
        .map_err(failure)
    }

    pub fn select_wiki_context(
        &self,
        room: &str,
        principal: &str,
        selection: &WikiContextSelection,
        at: u64,
    ) -> Result<(), WikiError> {
        if selection.slugs.is_empty()
            || selection.slugs.len() > 16
            || selection.reason.trim().is_empty()
            || selection.reason.len() > 1024
            || selection.slugs.iter().any(|s| {
                s.is_empty()
                    || s.len() > 80
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            })
            || selection
                .slugs
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != selection.slugs.len()
        {
            return Err(WikiError::InvalidInput);
        }
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let slugs = serde_json::to_string(&selection.slugs).map_err(|_| WikiError::InvalidInput)?;
        // Retrying exactly the same request never changes its selection fence.
        c.execute("INSERT INTO wiki_context_selection(room,principal,slugs,reason,requested_at) VALUES(?1,?2,?3,?4,?5)
            ON CONFLICT(room,principal) DO UPDATE SET slugs=excluded.slugs,reason=excluded.reason,
            version=wiki_context_selection.version+1,requested_at=excluded.requested_at
            WHERE wiki_context_selection.slugs!=excluded.slugs OR wiki_context_selection.reason!=excluded.reason",
            params![room,principal,slugs,selection.reason,at]).map_err(failure)?;
        Ok(())
    }

    pub fn wiki_context_for(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<serde_json::Value, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let revision: u64 = c
            .query_row(
                "SELECT revision FROM loca_wiki WHERE room=?1",
                params![room],
                |r| r.get(0),
            )
            .optional()
            .map_err(failure)?
            .ok_or(WikiError::NotConfigured)?;
        let selection: Option<(String, u64)> = c
            .query_row(
                "SELECT slugs,version FROM wiki_context_selection WHERE room=?1 AND principal=?2",
                params![room, principal],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(failure)?;
        let (slugs, version) = match selection {
            Some((raw, v)) => (
                serde_json::from_str::<Vec<String>>(&raw).map_err(|_| WikiError::Storage)?,
                v,
            ),
            None => (vec!["overview".into(), "working".into()], 0),
        };
        let mut pages = Vec::new();
        let mut missing = Vec::new();
        for slug in &slugs {
            let value: Option<(String,String,String,u64)> = c.query_row("SELECT title,body,sources,revision FROM loca_wiki_pages WHERE room=?1 AND slug=?2",
                params![room,slug],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(failure)?;
            match value {
                Some((title,body,sources,rev)) if rev>0 => pages.push(serde_json::json!({"slug":slug,"title":title,"body":body,
                    "sources":serde_json::from_str::<Vec<u64>>(&sources).map_err(|_|WikiError::Storage)?,"revision":rev})),
                _ => missing.push(slug),
            }
        }
        Ok(
            serde_json::json!({"room":room,"revision":revision,"selection_version":version,"slugs":slugs,"pages":pages,"missing":missing}),
        )
    }

    pub fn accept_wiki_context(
        &self,
        room: &str,
        principal: &str,
        ack: &WikiContextAck,
        at: u64,
    ) -> Result<(), WikiError> {
        if ack.context_id.is_empty()
            || ack.context_id.len() > 256
            || ack.packet_id.len() != 64
            || !ack.packet_id.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(WikiError::InvalidInput);
        }
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(failure)?;
        let prior: Option<(u64, u64)> = tx
            .query_row(
                "SELECT revision,selection_version FROM wiki_context_receipts
            WHERE room=?1 AND principal=?2 AND context_id=?3 AND packet_id=?4",
                params![room, principal, ack.context_id, ack.packet_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(failure)?;
        if let Some(value) = prior {
            return if value == (ack.revision, ack.selection_version) {
                Ok(())
            } else {
                Err(WikiError::Conflict)
            };
        }
        let current: Option<u64> = tx
            .query_row(
                "SELECT revision FROM loca_wiki WHERE room=?1",
                params![room],
                |r| r.get(0),
            )
            .optional()
            .map_err(failure)?;
        let version: u64 = tx
            .query_row(
                "SELECT version FROM wiki_context_selection WHERE room=?1 AND principal=?2",
                params![room, principal],
                |r| r.get(0),
            )
            .optional()
            .map_err(failure)?
            .unwrap_or(0);
        if current != Some(ack.revision) || version != ack.selection_version {
            return Err(WikiError::Conflict);
        }
        tx.execute(
            "INSERT INTO wiki_context_receipts VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                room,
                principal,
                ack.context_id,
                ack.packet_id,
                ack.revision,
                ack.selection_version,
                at
            ],
        )
        .map_err(failure)?;
        tx.execute("DELETE FROM wiki_context_receipts WHERE room=?1 AND principal=?2 AND rowid NOT IN
            (SELECT rowid FROM wiki_context_receipts WHERE room=?1 AND principal=?2 ORDER BY rowid DESC LIMIT 128)",params![room,principal]).map_err(failure)?;
        tx.commit().map_err(failure)
    }
}
