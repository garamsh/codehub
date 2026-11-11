use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::port::driven::draft_store::{DraftStore, DraftStoreError};

use crate::lock;

/// A draft store in memory.
#[derive(Default)]
pub struct MemoryDraftStore {
    drafts: Mutex<BTreeMap<Address, Changeset>>,
}

impl DraftStore for MemoryDraftStore {
    async fn put(&self, draft: Changeset) -> Result<Address, DraftStoreError> {
        let address = draft.id().map_err(DraftStoreError::Encode)?;
        lock(&self.drafts).insert(address, draft);
        Ok(address)
    }

    async fn get(&self, address: Address) -> Result<Changeset, DraftStoreError> {
        lock(&self.drafts)
            .get(&address)
            .cloned()
            .ok_or(DraftStoreError::NotFound(address))
    }
}
