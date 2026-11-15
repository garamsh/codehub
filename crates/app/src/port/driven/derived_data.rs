//! Derived data: the values and objects derived for changesets, rebuildable from the log
//! (ADR 0011).

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::file_version::FileVersionNode;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::domain::tree::Tree;

/// Holds each derived type's value for a changeset and the objects its steps wrote, per
/// repository.
#[async_trait::async_trait]
pub trait DerivedData: Send + Sync {
    /// Records `root` as `changeset`'s root tree, and stores `trees` by address.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    async fn put_trees(
        &self,
        repository: RepositoryId,
        changeset: Address,
        root: Address,
        trees: BTreeMap<Address, Tree>,
    ) -> Result<(), DerivedDataError>;

    /// The address of `changeset`'s root tree.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no root is recorded for `changeset`.
    async fn root(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Address, DerivedDataError>;

    /// The tree stored under `address`.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no tree is stored under `address`.
    async fn tree(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Tree, DerivedDataError>;

    /// Records `value` as `changeset`'s file-version nodes by path, and stores `nodes` by
    /// address.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    async fn put_file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
        value: BTreeMap<Path, Address>,
        nodes: BTreeMap<Address, FileVersionNode>,
    ) -> Result<(), DerivedDataError>;

    /// `changeset`'s file-version nodes by path; `None` where they have not been derived.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    async fn file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Option<BTreeMap<Path, Address>>, DerivedDataError>;

    /// The file-version node stored under `address`.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no node is stored under `address`.
    async fn file_version_node(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<FileVersionNode, DerivedDataError>;
}

/// Why derived data did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DerivedDataError {
    /// Nothing is recorded under the address.
    NotFound(Address),
    /// The store's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for DerivedDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(address) => write!(f, "no derived data for {address}"),
            Self::Backend(description) => write!(f, "derived data failed: {description}"),
        }
    }
}

impl std::error::Error for DerivedDataError {}
