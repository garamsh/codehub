use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::draft::{Draft, DraftId, Edit};
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
                description: String::new(),
                changes: BTreeMap::new(),
            })
            .changes
            .insert(path, change);
        Ok(())
    }

    async fn open(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
    ) -> Result<Draft, DraftStoreError> {
        Ok(lock(&self.drafts)
            .entry((repository, draft.clone()))
            .or_insert_with(|| Draft {
                base,
                description: String::new(),
                changes: BTreeMap::new(),
            })
            .clone())
    }

    async fn describe(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        description: String,
    ) -> Result<(), DraftStoreError> {
        lock(&self.drafts)
            .get_mut(&(repository, draft.clone()))
            .map(|held| held.description = description)
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))
    }

    async fn record(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), DraftStoreError> {
        let mut drafts = lock(&self.drafts);
        let held = drafts
            .get_mut(&(repository, draft.clone()))
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))?;
        for (path, edit) in edits {
            match edit {
                Edit::Record(change) => {
                    held.changes.insert(path, change);
                }
                Edit::Restore => {
                    held.changes.remove(&path);
                }
            }
        }
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
