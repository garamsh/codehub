//! The draft store: pending work, staged path by path and held outside the log until it lands
//! (ADR 0012).

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::draft::{Draft, DraftId, Edit, Instance};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;

/// Holds each repository's drafts, each under its [`DraftId`].
#[async_trait::async_trait]
pub trait DraftStore: Send + Sync {
    /// The draft held under `draft`, created with `base` as its base and no changes where none is
    /// held; a draft already held is returned as it is, keeping its own base.
    ///
    /// # Errors
    ///
    /// The store's [`DraftStoreError`].
    async fn open(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        base: Option<Address>,
    ) -> Result<Draft, DraftStoreError>;

    /// Sets the description of the draft held under `draft`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    async fn describe(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        description: String,
    ) -> Result<(), DraftStoreError>;

    /// Applies every edit to the draft held under `draft`, or none of them, only where it is the
    /// instance `instance` and its version is `expected`, and then raises the version by one.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft` or the one held is
    /// another instance, and [`DraftStoreError::Moved`] where its version is not `expected`;
    /// nothing is applied.
    async fn record(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        instance: Instance,
        expected: u64,
        edits: BTreeMap<Path, Edit>,
    ) -> Result<(), DraftStoreError>;

    /// The draft held under `draft`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft`.
    async fn get(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
    ) -> Result<Draft, DraftStoreError>;

    /// Removes the draft held under `draft`, only where it is the instance `instance` and its
    /// version is `expected`.
    ///
    /// # Errors
    ///
    /// [`DraftStoreError::NotFound`] where no draft is held under `draft` or the one held is
    /// another instance, and [`DraftStoreError::Moved`] where its version is not `expected`; the
    /// draft stays.
    async fn remove(
        &self,
        repository: RepositoryId,
        draft: &DraftId,
        instance: Instance,
        expected: u64,
    ) -> Result<(), DraftStoreError>;
}

/// Why the draft store refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DraftStoreError {
    /// No draft is held under the identity.
    NotFound(DraftId),
    /// The draft's version is not the one expected.
    Moved {
        /// The version the draft holds.
        current: u64,
    },
    /// The store's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for DraftStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(draft) => write!(f, "no draft {draft}"),
            Self::Moved { current } => write!(f, "the draft is at version {current}"),
            Self::Backend(description) => write!(f, "draft store failed: {description}"),
        }
    }
}

impl std::error::Error for DraftStoreError {}
