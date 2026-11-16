//! The cache refs: for each cache path of a check, the tree a passing run left there and the
//! changeset that run belongs to (ADR 0049); and for each cache path of a workspace instance, the
//! tree its latest run left (ADR 0051).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::draft::Instance;
use crate::domain::job::CacheRef;
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;

/// Holds the cache refs.
#[async_trait::async_trait]
pub trait CacheRefs: Send + Sync {
    /// The cache ref of `path`, which is relative to `folder`, in `repository`; `None` where it
    /// has none.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> Result<Option<CacheRef>, CacheRefsError>;

    /// Sets the cache ref of `path`, which is relative to `folder`, in `repository` to `to`,
    /// where it records `from` as its changeset, or none where `from` is `None`. Returns whether
    /// it did; `false` where the ref moved, which leaves it as it is.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn advance_cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
        from: Option<Address>,
        to: CacheRef,
    ) -> Result<bool, CacheRefsError>;

    /// The tree of the workspace cache ref of `path`, which is relative to the repository root, of
    /// `instance` in `repository`; `None` where it has none.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
    ) -> Result<Option<Address>, CacheRefsError>;

    /// Creates the workspace cache ref of `path` of `instance` in `repository` at `tree`. Returns
    /// whether it did; `false` where the ref exists, which leaves it as it is.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn create_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        tree: Address,
    ) -> Result<bool, CacheRefsError>;

    /// Moves the workspace cache ref of `path` of `instance` in `repository` to `to`, where it
    /// holds `from`. Returns whether it did; `false` where the ref holds another tree or none,
    /// which leaves it as it is.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn move_workspace_cache_ref(
        &self,
        repository: RepositoryId,
        instance: Instance,
        path: &Path,
        from: Address,
        to: Address,
    ) -> Result<bool, CacheRefsError>;

    /// Removes every workspace cache ref of `instance` in `repository`, and no other.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    async fn remove_workspace_cache_refs(
        &self,
        repository: RepositoryId,
        instance: Instance,
    ) -> Result<(), CacheRefsError>;
}

/// Why the cache refs did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum CacheRefsError {
    /// The store's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for CacheRefsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(description) => write!(f, "cache refs failed: {description}"),
        }
    }
}

impl std::error::Error for CacheRefsError {}
