//! Loca memory and the Loca Wiki.

use super::*;

impl Hub {
    pub fn loca_memory(&self, room: &str) -> rusqlite::Result<Option<protocol::LocaMemory>> {
        self.store.loca_memory(room)
    }

    pub fn loca_memory_version(&self, room: &str) -> rusqlite::Result<u64> {
        self.store.loca_memory_version(room)
    }

    pub fn loca_memory_snapshot(
        &self,
        room: &str,
    ) -> rusqlite::Result<Option<protocol::LocaMemorySnapshot>> {
        self.store.loca_memory_snapshot(room)
    }

    pub fn memory_persistence_available(&self) -> bool {
        self.store.memory_persistence_available()
    }

    pub fn wiki_snapshot(
        &self,
        room: &str,
    ) -> Result<crate::store::WikiSnapshot, crate::store::WikiError> {
        self.store.wiki_snapshot(room)
    }

    pub fn tick_wiki(&self) {
        if !self.store.is_persistent() {
            return;
        }
        let result = self.store.wiki_maintenance_rooms();
        match result {
            Ok(rooms) => {
                for room in rooms {
                    let result = if self.is_writable(&room) {
                        self.store
                            .schedule_wiki_review(&room, self.now())
                            .map(|_| ())
                    } else {
                        self.store.cancel_wiki_review(&room)
                    };
                    if let Err(error) = result {
                        tracing::warn!(?error, "Wiki maintenance scheduling failed");
                    }
                }
            }
            Err(error) => tracing::warn!(?error, "Wiki maintenance scan failed"),
        }
    }

    pub fn wiki_page(
        &self,
        room: &str,
        slug: &str,
    ) -> Result<crate::store::WikiPageSnapshot, crate::store::WikiError> {
        self.store.wiki_page(room, slug)
    }

    pub fn claim_wiki_review(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<Option<crate::store::WikiBatch>, crate::store::WikiError> {
        self.store.claim_wiki_review(room, principal, self.now())
    }

    pub fn finish_wiki_review(
        &self,
        room: &str,
        principal: &str,
        update: &crate::store::WikiCommit,
        lease: &crate::store::WikiLease,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .finish_wiki_review(room, principal, update, lease, self.now())
    }

    pub fn wiki_batch_status(&self, room: &str) -> Result<Option<String>, crate::store::WikiError> {
        self.store.wiki_batch_status(room)
    }

    pub fn renew_wiki_review(
        &self,
        room: &str,
        principal: &str,
        lease: &crate::store::WikiLease,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .renew_wiki_review(room, principal, lease, self.now())
    }

    pub fn configure_wiki(
        &self,
        room: &str,
        editor: &str,
        enabled: bool,
        interval: u64,
    ) -> Result<(), crate::store::WikiError> {
        let identities = self
            .store
            .active_principals_named(editor)
            .map_err(|_| crate::store::WikiError::Storage)?;
        let [identity] = identities.as_slice() else {
            return Err(crate::store::WikiError::InvalidInput);
        };
        if identity.kind != SenderType::Agent {
            return Err(crate::store::WikiError::InvalidInput);
        }
        if enabled
            && !self
                .store
                .wiki_editor_runtime_ready(room, &identity.id, self.now())?
        {
            return Err(crate::store::WikiError::Conflict);
        }
        self.store
            .configure_wiki(room, &identity.id, enabled, interval)
    }

    pub fn wiki_native_ready(
        &self,
        room: &str,
        principal: &str,
        runtime: &str,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .wiki_native_ready(room, principal, runtime, self.now())
    }

    pub fn wiki_context_for(
        &self,
        room: &str,
        principal: &str,
    ) -> Result<serde_json::Value, crate::store::WikiError> {
        self.store.wiki_context_for(room, principal)
    }

    pub fn wiki_context_requests(
        &self,
        room: &str,
        principal: Option<&str>,
        operator: bool,
    ) -> Result<serde_json::Value, crate::store::WikiError> {
        if !operator && principal != Some(self.wiki_snapshot(room)?.editor_principal.as_str()) {
            return Err(crate::store::WikiError::NotEditor);
        }
        let mut value = self.store.wiki_context_requests(room)?;
        let invited = self.invites_of(room);
        if let Some(requests) = value["requests"].as_array_mut() {
            requests.retain(|r| {
                invited
                    .iter()
                    .any(|i| Some(i.name.as_str()) == r["target"].as_str())
            });
        }
        Ok(value)
    }

    pub fn select_wiki_context(
        &self,
        room: &str,
        principal: &str,
        selection: &crate::store::WikiContextSelection,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .select_wiki_context(room, principal, selection, self.now())
    }

    pub fn accept_wiki_context(
        &self,
        room: &str,
        principal: &str,
        ack: &crate::store::WikiContextAck,
    ) -> Result<(), crate::store::WikiError> {
        self.store
            .accept_wiki_context(room, principal, ack, self.now())
    }

    pub fn offer_wiki_context(
        &self,
        room: &str,
        actor: Option<&str>,
        operator: bool,
        target: &str,
        selection: &crate::store::WikiContextSelection,
    ) -> Result<(), crate::store::WikiError> {
        let snapshot = self.wiki_snapshot(room)?;
        if !operator && actor != Some(snapshot.editor_principal.as_str()) {
            return Err(crate::store::WikiError::NotEditor);
        }
        let principals = self
            .store
            .active_principals_named(target)
            .map_err(|_| crate::store::WikiError::Storage)?;
        let [identity] = principals.as_slice() else {
            return Err(crate::store::WikiError::InvalidInput);
        };
        if identity.kind != SenderType::Agent
            || !self.invites_of(room).iter().any(|i| i.name == target)
        {
            return Err(crate::store::WikiError::InvalidInput);
        }
        self.select_wiki_context(room, &identity.id, selection)
    }

    pub fn commit_wiki(
        &self,
        room: &str,
        principal: &str,
        update: &crate::store::WikiCommit,
    ) -> Result<u64, crate::store::WikiError> {
        self.store.commit_wiki(room, principal, update, self.now())
    }

    pub fn remove_wiki_page(
        &self,
        room: &str,
        principal: &str,
        slug: &str,
        removal: &crate::store::WikiPageRemoval,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .remove_wiki_page(room, principal, slug, removal, self.now())
    }

    pub fn edit_wiki_page(
        &self,
        room: &str,
        actor: &str,
        slug: &str,
        edit: &crate::store::WikiPageEdit,
    ) -> Result<u64, crate::store::WikiError> {
        self.store
            .edit_wiki_page(room, actor, slug, edit, self.now())
    }

    pub fn loca_memory_metadata(&self) -> rusqlite::Result<Vec<protocol::LocaMemoryMetadata>> {
        self.store.loca_memory_metadata()
    }

    pub fn loca_memory_entries(
        &self,
        room: &str,
        after_id: u64,
        limit: usize,
    ) -> Result<protocol::LocaMemoryEntryPage, crate::store::MemoryReadError> {
        self.store.loca_memory_entries(room, after_id, limit)
    }
}
