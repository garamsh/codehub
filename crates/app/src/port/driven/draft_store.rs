//! The draft store: pending work, staged path by path and held outside the log until it lands
//! (ADR 0012).

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::{Draft, DraftId};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;

/// Holds each repository's drafts, each under its [`DraftId`].
pub trait DraftStore {
    /// Records `change` at `path` in the draft, replacing an earlier change to the path. A draft
    /// not yet held is created with `base` as its base; a draft already held keeps its own.
    ///
    /// # Errors
    ///
    /// The store's [`DraftStoreError`].
    fn stage(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
        path: Path,
        change: Change,
    ) -> impl Future<Output = Result<(), DraftStoreError>> + Send;

    /// The draft held under `draft`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    fn get(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> impl Future<Output = Result<Draft, DraftStoreError>> + Send;

    /// Removes the draft held under `draft`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    fn remove(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> impl Future<Output = Result<(), DraftStoreError>> + Send;
}

/// Why the draft store refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftStoreError {
    /// No draft is held under the identity.
    NotFound(DraftId),
}

impl fmt::Display for DraftStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(draft) => write!(f, "no draft {draft}"),
        }
    }
}

impl std::error::Error for DraftStoreError {}
