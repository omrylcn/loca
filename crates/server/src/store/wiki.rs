//! Durable wiki state. Access to reads and operator configuration belongs to
//! the HTTP layer; writes additionally bind to the assigned principal here.
use super::*;
use serde::{Deserialize, Serialize};

const PAGE_MAX_BYTES: usize = 32 * 1024;
const COMMIT_MAX_PAGES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WikiError {
    PersistenceUnavailable,
    NotConfigured,
    NotEditor,
    Conflict,
    InvalidInput,
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WikiPage {
    pub slug: String,
    pub title: String,
    pub body: String,
    pub sources: Vec<u64>,
    #[serde(default)]
    pub revision: u64,
}

#[derive(Debug, Serialize)]
pub struct WikiSnapshot {
    pub room: String,
    pub editor_principal: String,
    pub editor_name: Option<String>,
    pub enabled: bool,
    pub interval_messages: u64,
    pub revision: u64,
    pub reviewed_through: u64,
    pub reviewed_at: Option<u64>,
    pub edited_at: Option<u64>,
    pub pages: Vec<WikiPage>,
}

#[derive(Debug, Deserialize)]
pub struct WikiCommit {
    pub expected_revision: u64,
    pub reviewed_through: u64,
    pub reason: String,
    pub pages: Vec<WikiPage>,
}

fn storage(_: rusqlite::Error) -> WikiError {
    WikiError::Storage
}

fn pages(c: &Connection, room: &str) -> Result<Vec<WikiPage>, WikiError> {
    let mut stmt = c
        .prepare(
            "SELECT slug, title, body, sources, revision FROM loca_wiki_pages
         WHERE room = ?1 ORDER BY slug",
        )
        .map_err(storage)?;
    let rows = stmt
        .query_map(params![room], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, u64>(4)?,
            ))
        })
        .map_err(storage)?;
    rows.map(|r| {
        let (slug, title, body, sources, revision) = r.map_err(storage)?;
        Ok(WikiPage {
            slug,
            title,
            body,
            sources: serde_json::from_str(&sources).map_err(|_| WikiError::Storage)?,
            revision,
        })
    })
    .collect()
}

impl Store {
    /// The caller must verify operator authority and resolve an active agent's
    /// principal. Reassigning never resets content or review progress.
    pub fn configure_wiki(
        &self,
        room: &str,
        editor: &str,
        enabled: bool,
        interval: u64,
    ) -> Result<(), WikiError> {
        if editor.trim().is_empty() || !(1..=1000).contains(&interval) {
            return Err(WikiError::InvalidInput);
        }
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(storage)?;
        tx.execute(
            "INSERT INTO loca_wiki(room, editor_principal, enabled, interval_messages)
            VALUES(?1, ?2, ?3, ?4) ON CONFLICT(room) DO UPDATE SET
            editor_principal=excluded.editor_principal, enabled=excluded.enabled,
            interval_messages=excluded.interval_messages",
            params![room, editor, enabled, interval],
        )
        .map_err(storage)?;
        for (slug, title) in [("overview", "Overview"), ("working", "Working area")] {
            tx.execute(
                "INSERT OR IGNORE INTO loca_wiki_pages
                (room, slug, title, body, sources, revision) VALUES(?1, ?2, ?3, '', '[]', 0)",
                params![room, slug, title],
            )
            .map_err(storage)?;
        }
        tx.commit().map_err(storage)
    }

    pub fn wiki_snapshot(&self, room: &str) -> Result<WikiSnapshot, WikiError> {
        let c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let mut snapshot = c
            .query_row(
                "SELECT editor_principal, enabled, interval_messages,
            revision, reviewed_through, reviewed_at, edited_at,
            (SELECT display_name FROM principals WHERE id=editor_principal AND disabled_at IS NULL)
            FROM loca_wiki WHERE room=?1",
                params![room],
                |r| {
                    Ok(WikiSnapshot {
                        room: room.into(),
                        editor_principal: r.get(0)?,
                        editor_name: r.get(7)?,
                        enabled: r.get(1)?,
                        interval_messages: r.get(2)?,
                        revision: r.get(3)?,
                        reviewed_through: r.get(4)?,
                        reviewed_at: r.get(5)?,
                        edited_at: r.get(6)?,
                        pages: Vec::new(),
                    })
                },
            )
            .optional()
            .map_err(storage)?
            .ok_or(WikiError::NotConfigured)?;
        snapshot.pages = pages(&c, room)?;
        Ok(snapshot)
    }

    pub fn commit_wiki(
        &self,
        room: &str,
        principal: &str,
        update: &WikiCommit,
        at: u64,
    ) -> Result<u64, WikiError> {
        if update.pages.len() > COMMIT_MAX_PAGES
            || update.reason.trim().is_empty()
            || update.reason.len() > 4096
        {
            return Err(WikiError::InvalidInput);
        }
        let mut c = self.conn().ok_or(WikiError::PersistenceUnavailable)?;
        let tx = c.transaction().map_err(storage)?;
        let (editor, revision, cursor): (String, u64, u64) = tx
            .query_row(
                "SELECT editor_principal, revision, reviewed_through FROM loca_wiki WHERE room=?1",
                params![room],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(storage)?
            .ok_or(WikiError::NotConfigured)?;
        if editor != principal {
            return Err(WikiError::NotEditor);
        }
        if update.expected_revision != revision || update.reviewed_through < cursor {
            return Err(WikiError::Conflict);
        }
        // A cursor cannot claim a message from another room or a nonexistent one.
        if update.reviewed_through != 0
            && !tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE room=?1 AND id=?2)",
                    params![room, update.reviewed_through],
                    |r| r.get::<_, bool>(0),
                )
                .map_err(storage)?
        {
            return Err(WikiError::InvalidInput);
        }
        let existing = pages(&tx, room)?;
        let mut changed = Vec::new();
        let mut slugs = std::collections::HashSet::new();
        for page in &update.pages {
            if page.slug.is_empty()
                || page.slug.len() > 80
                || !page
                    .slug
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                || !slugs.insert(&page.slug)
                || page.title.trim().is_empty()
                || page.title.len() > 256
                || page.body.len() > PAGE_MAX_BYTES
                || page.sources.len() > 256
            {
                return Err(WikiError::InvalidInput);
            }
            for source in &page.sources {
                if *source > update.reviewed_through
                    || !tx
                        .query_row(
                            "SELECT EXISTS(SELECT 1 FROM messages WHERE room=?1 AND id=?2)",
                            params![room, source],
                            |r| r.get::<_, bool>(0),
                        )
                        .map_err(storage)?
                {
                    return Err(WikiError::InvalidInput);
                }
            }
            if !existing.iter().any(|p| {
                p.slug == page.slug
                    && p.title == page.title
                    && p.body == page.body
                    && p.sources == page.sources
            }) {
                changed.push(page);
            }
        }
        let next = revision
            .checked_add(u64::from(!changed.is_empty()))
            .ok_or(WikiError::Storage)?;
        for page in &changed {
            let sources = serde_json::to_string(&page.sources).map_err(|_| WikiError::Storage)?;
            tx.execute(
                "INSERT INTO loca_wiki_pages(room, slug, title, body, sources, revision)
                VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(room, slug) DO UPDATE SET
                title=excluded.title, body=excluded.body, sources=excluded.sources,
                revision=excluded.revision",
                params![room, page.slug, page.title, page.body, sources, next],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO loca_wiki_history
                (room, slug, revision, title, body, sources, editor_principal, reason, edited_at)
                VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    room,
                    page.slug,
                    next,
                    page.title,
                    page.body,
                    sources,
                    principal,
                    update.reason,
                    at
                ],
            )
            .map_err(storage)?;
        }
        tx.execute(
            "UPDATE loca_wiki SET revision=?2, reviewed_through=?3, reviewed_at=?4,
            edited_at=CASE WHEN ?5 THEN ?4 ELSE edited_at END WHERE room=?1",
            params![room, next, update.reviewed_through, at, !changed.is_empty()],
        )
        .map_err(storage)?;
        tx.commit().map_err(storage)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn store() -> Store {
        Store::open(Some(":memory:")).unwrap()
    }
    fn commit(rev: u64, body: &str) -> WikiCommit {
        WikiCommit {
            expected_revision: rev,
            reviewed_through: 0,
            reason: "review".into(),
            pages: vec![WikiPage {
                slug: "working".into(),
                title: "Working area".into(),
                body: body.into(),
                sources: vec![],
                revision: 0,
            }],
        }
    }
    #[test]
    fn wiki_no_change_does_not_increment_content_revision() {
        let s = store();
        s.configure_wiki("a", "p1", false, 30).unwrap();
        assert_eq!(s.commit_wiki("a", "p1", &commit(0, "proposal"), 1), Ok(1));
        assert_eq!(s.commit_wiki("a", "p1", &commit(1, "proposal"), 2), Ok(1));
        let snap = s.wiki_snapshot("a").unwrap();
        assert_eq!(snap.reviewed_at, Some(2));
        assert_eq!(snap.edited_at, Some(1));
    }
    #[test]
    fn wiki_reassignment_revokes_previous_editor_without_erasing_pages() {
        let s = store();
        s.configure_wiki("a", "p1", true, 30).unwrap();
        s.commit_wiki("a", "p1", &commit(0, "saved"), 1).unwrap();
        s.configure_wiki("a", "p2", false, 10).unwrap();
        assert_eq!(
            s.commit_wiki("a", "p1", &commit(1, "bad"), 2),
            Err(WikiError::NotEditor)
        );
        assert_eq!(s.wiki_snapshot("a").unwrap().revision, 1);
    }
    #[test]
    fn wiki_conflict_and_invalid_batch_are_atomic() {
        let s = store();
        s.configure_wiki("a", "p1", false, 30).unwrap();
        s.commit_wiki("a", "p1", &commit(0, "saved"), 1).unwrap();
        assert_eq!(
            s.commit_wiki("a", "p1", &commit(0, "stale"), 2),
            Err(WikiError::Conflict)
        );
        let mut update = commit(1, "new");
        let mut invalid = update.pages[0].clone();
        invalid.slug = "topic".into();
        invalid.sources = vec![999];
        update.pages.push(invalid);
        assert_eq!(
            s.commit_wiki("a", "p1", &update, 3),
            Err(WikiError::InvalidInput)
        );
        let snap = s.wiki_snapshot("a").unwrap();
        assert_eq!(snap.revision, 1);
        assert_eq!(snap.reviewed_at, Some(1));
        assert!(snap.pages.iter().any(|p| p.body == "saved"));
    }
}
