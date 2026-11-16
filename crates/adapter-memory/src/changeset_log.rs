use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};

use crate::lock;

/// A changeset log in memory.
#[derive(Default)]
pub struct MemoryChangesetLog {
    changesets: Mutex<BTreeMap<(RepositoryId, Address), Changeset>>,
    refs: Mutex<BTreeMap<(RepositoryId, Ref), Address>>,
}

#[async_trait::async_trait]
impl ChangesetLog for MemoryChangesetLog {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        let id = changeset.id().map_err(ChangesetLogError::Encode)?;
        lock(&self.changesets).insert((repository, id), changeset);
        Ok(id)
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        lock(&self.changesets)
            .get(&(repository, id))
            .cloned()
            .ok_or(ChangesetLogError::NotFound(id))
    }

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        let changesets = lock(&self.changesets);
        let mut chain = Vec::new();
        let mut at = head;
        while at != base {
            let Some(id) = at else {
                return match base {
                    Some(base) if !changesets.contains_key(&(repository, base)) => {
                        Err(ChangesetLogError::NotFound(base))
                    }
                    _ => Ok(None),
                };
            };
            if chain.len() == limit {
                return Err(ChangesetLogError::Backend(format!(
                    "the first-parent chain is longer than {limit}"
                )));
            }
            let changeset = changesets
                .get(&(repository, id))
                .ok_or(ChangesetLogError::NotFound(id))?;
            at = changeset.parents.first().copied();
            chain.push(changeset.clone());
        }
        Ok(Some(chain))
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        Ok(lock(&self.refs).get(&(repository, name)).copied())
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        let mut refs = lock(&self.refs);
        let current = refs.get(&(repository, name)).copied();
        if current != expected {
            return Err(ChangesetLogError::RefMoved { current });
        }
        refs.insert((repository, name), new);
        Ok(())
    }
}
