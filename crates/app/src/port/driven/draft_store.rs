//! The draft store: pending work, staged path by path and held outside the log until it lands
//! (ADR 0012).

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::draft::{Draft, DraftId, Edit};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;

/// Holds each repository's drafts, each under its [`DraftId`].
pub trait DraftStore {
    /// The draft held under `draft`, created with `base` as its base and no changes where none is
    /// held; a draft already held is returned as it is, keeping its own base.
    ///
    /// # Errors
    ///
    /// The store's [`DraftStoreError`].
    fn open(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
    ) -> impl Future<Output = Result<Draft, DraftStoreError>> + Send;

    /// Sets the description of the draft held under `draft`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    fn describe(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        description: String,
    ) -> impl Future<Output = Result<(), DraftStoreError>> + Send;

    /// Applies every edit to the draft held under `draft`, or none of them.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    fn record(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        edits: BTreeMap<Path, Edit>,
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
    /// The store's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for DraftStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(draft) => write!(f, "no draft {draft}"),
            Self::Backend(description) => write!(f, "draft store failed: {description}"),
        }
    }
}

impl std::error::Error for DraftStoreError {}
