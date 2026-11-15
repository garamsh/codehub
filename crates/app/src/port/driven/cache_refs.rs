//! The cache refs: for each cache path of a check, the tree a passing run left there and the
//! changeset that run belongs to (ADR 0049).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::job::CacheRef;
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;

/// Holds the cache refs.
pub trait CacheRefs {
    /// The cache ref of `path`, which is relative to `folder`, in `repository`; `None` where it
    /// has none.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> impl Future<Output = Result<Option<CacheRef>, CacheRefsError>> + Send;

    /// Sets the cache ref of `path`, which is relative to `folder`, in `repository` to `to`,
    /// where it records `from` as its changeset, or none where `from` is `None`. Returns whether
    /// it did; `false` where the ref moved, which leaves it as it is.
    ///
    /// # Errors
    ///
    /// [`CacheRefsError::Backend`] where the store fails.
    fn advance_cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
        from: Option<Address>,
        to: CacheRef,
    ) -> impl Future<Output = Result<bool, CacheRefsError>> + Send;
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
