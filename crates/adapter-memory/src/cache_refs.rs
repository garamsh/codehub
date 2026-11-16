use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::draft::Instance;
use app::domain::job::CacheRef;
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::cache_refs::{CacheRefs, CacheRefsError};

use crate::lock;

/// The cache refs in memory.
#[derive(Default)]
pub struct MemoryCacheRefs {
    refs: Mutex<BTreeMap<(RepositoryId, Prefix, Path), CacheRef>>,
    workspace_refs: Mutex<BTreeMap<(RepositoryId, Instance, Path), Address>>,
}

#[async_trait::async_trait]
impl CacheRefs for MemoryCacheRefs {
    async fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> Result<Option<CacheRef>, CacheRefsError> {
        Ok(lock(&self.refs)
            .get(&(repository, folder.clone(), path.clone()))
            .copied())
    }

    async fn advance_cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
        from: Option<Address>,
        to: CacheRef,
    ) -> Result<bool, CacheRefsError> {
        let mut held = lock(&self.refs);
        let key = (repository, folder.clone(), path.clone());
        if held.get(&key).map(|held| held.changeset) != from {
            return Ok(false);
        }
        held.insert(key, to);
        Ok(true)
    }

    async fn workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
    ) -> Result<Option<Address>, CacheRefsError> {
        Ok(lock(&self.workspace_refs)
            .get(&(repository, instance, path.clone()))
            .copied())
    }

    async fn create_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        tree: Address,
    ) -> Result<bool, CacheRefsError> {
        match lock(&self.workspace_refs).entry((repository, instance, path.clone())) {
            Entry::Occupied(_) => Ok(false),
            Entry::Vacant(vacant) => {
                vacant.insert(tree);
                Ok(true)
            }
        }
    }

    async fn move_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        from: Address,
        to: Address,
    ) -> Result<bool, CacheRefsError> {
        let mut held = lock(&self.workspace_refs);
        match held.get_mut(&(repository, instance, path.clone())) {
            Some(tree) if *tree == from => {
                *tree = to;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn remove_workspace_cache_refs(
        &self,
        repository: RepositoryId,
        instance: Instance,
    ) -> Result<(), CacheRefsError> {
        lock(&self.workspace_refs)
            .retain(|(held, of, _), _| (*held, *of) != (repository, instance));
        Ok(())
    }
}
