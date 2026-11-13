//! Listing the files of a changeset's tree under a prefix (ADR 0048).

use crate::domain::address::Address;
use crate::domain::file::File;
use crate::domain::path::{Component, Path, Prefix};
use crate::domain::repository::RepositoryId;
use crate::domain::tree::Entry;
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};

/// Every file under `prefix` in the tree rooted at `root`, in path order; the file at `prefix`
/// itself where `prefix` names one.
///
/// # Errors
///
/// The failure of the derived-data port, [`DerivedDataError::NotFound`] where a tree is missing.
pub async fn files_under<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    root: Address,
    prefix: &Prefix,
) -> Result<Vec<(Path, File)>, DerivedDataError> {
    let mut entry = Entry::Directory(root);
    for component in prefix.components() {
        let Entry::Directory(tree) = entry else {
            return Ok(Vec::new());
        };
        match derived.tree(repository, tree).await?.entries.get(component) {
            Some(found) => entry = *found,
            None => return Ok(Vec::new()),
        }
    }

    let mut files = Vec::new();
    let mut pending: Vec<(Vec<Component>, Entry)> = vec![(prefix.components().to_vec(), entry)];
    while let Some((components, entry)) = pending.pop() {
        match entry {
            Entry::File(file) => {
                if let Ok(path) = Path::new(components) {
                    files.push((path, file));
                }
            }
            Entry::Directory(tree) => {
                for (name, child) in derived.tree(repository, tree).await?.entries {
                    let mut child_components = components.clone();
                    child_components.push(name);
                    pending.push((child_components, child));
                }
            }
        }
    }
    files.sort_by(|(left, _), (right, _)| left.cmp(right));
    Ok(files)
}
