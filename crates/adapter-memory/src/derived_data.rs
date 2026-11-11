use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::file_version::FileVersionNode;
use app::domain::path::Path;
use app::domain::repository::RepositoryId;
use app::domain::tree::Tree;
use app::port::driven::derived_data::{DerivedData, DerivedDataError};

use crate::lock;

/// Derived data in memory.
#[derive(Default)]
pub struct MemoryDerivedData {
    roots: Mutex<BTreeMap<(RepositoryId, Address), Address>>,
    trees: Mutex<BTreeMap<(RepositoryId, Address), Tree>>,
    file_versions: Mutex<BTreeMap<(RepositoryId, Address), BTreeMap<Path, Address>>>,
    file_version_nodes: Mutex<BTreeMap<(RepositoryId, Address), FileVersionNode>>,
}

impl DerivedData for MemoryDerivedData {
    async fn put_trees(
        &self,
        repository: RepositoryId,
        changeset: Address,
        root: Address,
        trees: BTreeMap<Address, Tree>,
    ) -> Result<(), DerivedDataError> {
        lock(&self.trees).extend(
            trees
                .into_iter()
                .map(|(address, tree)| ((repository, address), tree)),
        );
        lock(&self.roots).insert((repository, changeset), root);
        Ok(())
    }

    async fn root(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Address, DerivedDataError> {
        lock(&self.roots)
            .get(&(repository, changeset))
            .copied()
            .ok_or(DerivedDataError::NotFound(changeset))
    }

    async fn tree(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<Tree, DerivedDataError> {
        lock(&self.trees)
            .get(&(repository, address))
            .cloned()
            .ok_or(DerivedDataError::NotFound(address))
    }

    async fn put_file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
        value: BTreeMap<Path, Address>,
        nodes: BTreeMap<Address, FileVersionNode>,
    ) -> Result<(), DerivedDataError> {
        lock(&self.file_version_nodes).extend(
            nodes
                .into_iter()
                .map(|(address, node)| ((repository, address), node)),
        );
        lock(&self.file_versions).insert((repository, changeset), value);
        Ok(())
    }

    async fn file_versions(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Option<BTreeMap<Path, Address>>, DerivedDataError> {
        Ok(lock(&self.file_versions)
            .get(&(repository, changeset))
            .cloned())
    }

    async fn file_version_node(
        &self,
        repository: RepositoryId,
        address: Address,
    ) -> Result<FileVersionNode, DerivedDataError> {
        lock(&self.file_version_nodes)
            .get(&(repository, address))
            .cloned()
            .ok_or(DerivedDataError::NotFound(address))
    }
}
