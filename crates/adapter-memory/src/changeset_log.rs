use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};

use crate::lock;

/// A changeset log in memory.
#[derive(Default)]
pub struct MemoryChangesetLog {
    changesets: Mutex<BTreeMap<Address, Changeset>>,
    refs: Mutex<BTreeMap<Ref, Address>>,
}

impl ChangesetLog for MemoryChangesetLog {
    async fn append(&self, changeset: Changeset) -> Result<Address, ChangesetLogError> {
        let id = changeset.id().map_err(ChangesetLogError::Encode)?;
        lock(&self.changesets).insert(id, changeset);
        Ok(id)
    }

    async fn get(&self, id: Address) -> Result<Changeset, ChangesetLogError> {
        lock(&self.changesets)
            .get(&id)
            .cloned()
            .ok_or(ChangesetLogError::NotFound(id))
    }

    async fn read_ref(&self, name: Ref) -> Result<Option<Address>, ChangesetLogError> {
        Ok(lock(&self.refs).get(&name).copied())
    }

    async fn compare_and_swap(
        &self,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        let mut refs = lock(&self.refs);
        let current = refs.get(&name).copied();
        if current != expected {
            return Err(ChangesetLogError::RefMoved { current });
        }
        refs.insert(name, new);
        Ok(())
    }
}
