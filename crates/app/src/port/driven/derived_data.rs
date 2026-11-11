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
pub trait DerivedData {
    /// Records `root` as `changeset`'s root tree, and stores `trees` by address.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    fn put_trees(
        &self,
        repository: RepositoryId,
        changeset: Address,
        root: Address,
        trees: BTreeMap<Address, Tree>,
    ) -> impl Future<Output = Result<(), DerivedDataError>> + Send;

    /// The address of `changeset`'s root tree.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no root is recorded for `changeset`.
    fn root(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> impl Future<Output = Result<Address, DerivedDataError>> + Send;

    /// The tree stored under `address`.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no tree is stored under `address`.
    fn tree(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<Tree, DerivedDataError>> + Send;

    /// Records `value` as `changeset`'s file-version nodes by path, and stores `nodes` by
    /// address.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    fn put_file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
        value: BTreeMap<Path, Address>,
        nodes: BTreeMap<Address, FileVersionNode>,
    ) -> impl Future<Output = Result<(), DerivedDataError>> + Send;

    /// `changeset`'s file-version nodes by path; `None` where they have not been derived.
    ///
    /// # Errors
    ///
    /// The store's [`DerivedDataError`].
    fn file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> impl Future<Output = Result<Option<BTreeMap<Path, Address>>, DerivedDataError>> + Send;

    /// The file-version node stored under `address`.
    ///
    /// # Errors
    ///
    /// [`DerivedDataError::NotFound`] where no node is stored under `address`.
    fn file_version_node(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> impl Future<Output = Result<FileVersionNode, DerivedDataError>> + Send;
}

/// Why derived data did not answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DerivedDataError {
    /// Nothing is recorded under the address.
    NotFound(Address),
}

impl fmt::Display for DerivedDataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(address) => write!(f, "no derived data for {address}"),
        }
    }
}

impl std::error::Error for DerivedDataError {}
