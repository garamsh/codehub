use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::job::CacheRef;
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::port::driven::cache_refs::{CacheRefs, CacheRefsError};

use crate::lock;

/// The cache refs in memory.
#[derive(Default)]
pub struct MemoryCacheRefs {
    refs: Mutex<BTreeMap<(RepositoryId, Prefix, Path), CacheRef>>,
}

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
}
