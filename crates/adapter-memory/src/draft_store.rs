use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::draft::{Draft, DraftId};
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::port::driven::draft_store::{DraftStore, DraftStoreError};

use crate::lock;

/// A draft store in memory.
#[derive(Default)]
pub struct MemoryDraftStore {
    drafts: Mutex<BTreeMap<(RepositoryId, DraftId), Draft>>,
}

impl DraftStore for MemoryDraftStore {
    async fn stage(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
        path: Path,
        change: Change,
    ) -> Result<(), DraftStoreError> {
        lock(&self.drafts)
            .entry((repository, draft.clone()))
            .or_insert_with(|| Draft {
                base,
                changes: BTreeMap::new(),
            })
            .changes
            .insert(path, change);
        Ok(())
    }

    async fn get(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> Result<Draft, DraftStoreError> {
        lock(&self.drafts)
            .get(&(repository, draft.clone()))
            .cloned()
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))
    }

    async fn remove(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> Result<(), DraftStoreError> {
        lock(&self.drafts)
            .remove(&(repository, draft.clone()))
            .map(drop)
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))
    }
}
