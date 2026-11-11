use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::address::Address;
use app::domain::file_version::FileVersionNode;
use app::domain::path::Path;
use app::domain::tree::Tree;
use app::port::driven::derived_data::{DerivedData, DerivedDataError};

use crate::lock;

/// Derived data in memory.
#[derive(Default)]
pub struct MemoryDerivedData {
    roots: Mutex<BTreeMap<Address, Address>>,
    trees: Mutex<BTreeMap<Address, Tree>>,
    file_versions: Mutex<BTreeMap<Address, BTreeMap<Path, Address>>>,
    file_version_nodes: Mutex<BTreeMap<Address, FileVersionNode>>,
}

impl DerivedData for MemoryDerivedData {
    async fn put_trees(
        &self,
        changeset: Address,
        root: Address,
        trees: BTreeMap<Address, Tree>,
    ) -> Result<(), DerivedDataError> {
        lock(&self.trees).extend(trees);
        lock(&self.roots).insert(changeset, root);
        Ok(())
    }

    async fn root(&self, changeset: Address) -> Result<Address, DerivedDataError> {
        lock(&self.roots)
            .get(&changeset)
            .copied()
            .ok_or(DerivedDataError::NotFound(changeset))
    }

    async fn tree(&self, address: Address) -> Result<Tree, DerivedDataError> {
        lock(&self.trees)
            .get(&address)
            .cloned()
            .ok_or(DerivedDataError::NotFound(address))
    }

    async fn put_file_versions(
        &self,
        changeset: Address,
        value: BTreeMap<Path, Address>,
        nodes: BTreeMap<Address, FileVersionNode>,
    ) -> Result<(), DerivedDataError> {
        lock(&self.file_version_nodes).extend(nodes);
        lock(&self.file_versions).insert(changeset, value);
        Ok(())
    }

    async fn file_versions(
        &self,
        changeset: Address,
    ) -> Result<Option<BTreeMap<Path, Address>>, DerivedDataError> {
        Ok(lock(&self.file_versions).get(&changeset).cloned())
    }

    async fn file_version_node(
        &self,
        address: Address,
    ) -> Result<FileVersionNode, DerivedDataError> {
        lock(&self.file_version_nodes)
            .get(&address)
            .cloned()
            .ok_or(DerivedDataError::NotFound(address))
    }
}
