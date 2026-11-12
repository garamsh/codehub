//! Reading a file at `main`: its kind and a grant to download its content.

use crate::domain::address::Address;
use crate::domain::file::File;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::domain::tree::Entry;
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::source_control::{FileContent, ReadFileError};

/// The file at `path` in `repository`'s `main`.
///
/// # Errors
///
/// [`ReadFileError::NotFound`] where `main` is unset or holds no file at `path`, and otherwise
/// the failure of a port.
pub async fn read_file<O, L, D>(
    objects: &O,
    log: &L,
    derived: &D,
    repository: RepositoryId,
    path: &Path,
) -> Result<FileContent, ReadFileError>
where
    O: ObjectStore,
    L: ChangesetLog,
    D: DerivedData,
{
    let not_found = || ReadFileError::NotFound(path.clone());
    let main = log
        .read_ref(repository, Ref::Main)
        .await?
        .ok_or_else(not_found)?;
    let root = derived.root(repository, main).await?;
    let file = file_at(derived, repository, Some(root), path)
        .await?
        .ok_or_else(not_found)?;
    Ok(FileContent {
        kind: file.kind,
        download: objects.download_grant(repository, file.content).await?,
    })
}

/// The file at `path` in the tree rooted at `root`, if it holds one.
pub(crate) async fn file_at<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    root: Option<Address>,
    path: &Path,
) -> Result<Option<File>, DerivedDataError> {
    let Some(mut tree) = root else {
        return Ok(None);
    };
    let Some((name, directories)) = path.components().split_last() else {
        return Ok(None);
    };
    for directory in directories {
        match derived.tree(repository, tree).await?.entries.get(directory) {
            Some(Entry::Directory(subtree)) => tree = *subtree,
            Some(Entry::File(_)) | None => return Ok(None),
        }
    }
    match derived.tree(repository, tree).await?.entries.get(name) {
        Some(Entry::File(file)) => Ok(Some(*file)),
        Some(Entry::Directory(_)) | None => Ok(None),
    }
}
