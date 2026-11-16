use std::collections::BTreeMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use app::domain::address::Address;
use app::domain::draft::{Draft, DraftId, Edit, Instance};
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::port::driven::draft_store::{DraftStore, DraftStoreError};

use crate::lock;

/// A draft store in memory.
#[derive(Default)]
pub struct MemoryDraftStore {
    drafts: Mutex<BTreeMap<(RepositoryId, DraftId), Draft>>,
    instances: AtomicU64,
}

#[async_trait::async_trait]
impl DraftStore for MemoryDraftStore {
    async fn open(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
    ) -> Result<Draft, DraftStoreError> {
        Ok(lock(&self.drafts)
            .entry((repository, draft.clone()))
            .or_insert_with(|| Draft {
                instance: Instance::new(self.instances.fetch_add(1, Ordering::Relaxed)),
                version: 0,
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
        instance: Instance,
        expected: u64,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), DraftStoreError> {
        let mut drafts = lock(&self.drafts);
        let held = drafts
            .get_mut(&(repository, draft.clone()))
            .filter(|held| held.instance == instance)
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))?;
        if held.version != expected {
            return Err(DraftStoreError::Moved {
                current: held.version,
            });
        }
        held.version += 1;
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
        instance: Instance,
        expected: u64,
    ) -> Result<(), DraftStoreError> {
        let mut drafts = lock(&self.drafts);
        let key = (repository, draft.clone());
        let held = drafts
            .get(&key)
            .filter(|held| held.instance == instance)
            .ok_or_else(|| DraftStoreError::NotFound(draft.clone()))?;
        if held.version != expected {
            return Err(DraftStoreError::Moved {
                current: held.version,
            });
        }
        drafts.remove(&key);
        Ok(())
    }
}
