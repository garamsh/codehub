//! Staging: one path's change recorded in a draft, the first staging basing the draft on `main`.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::draft::DraftId;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use crate::port::driven::draft_store::{DraftStore, DraftStoreError};
use crate::port::driven::object_store::{ObjectStore, ObjectStoreError};

/// Records `change` at `path` in the draft `draft` of `repository`, creating the draft on `main`
/// as it stands now where it does not exist.
///
/// # Errors
///
/// [`StageError::ObjectMissing`] where the change writes a file whose content is not in the
/// repository's object store, and otherwise the failure of a port.
pub async fn stage<O, L, S>(
    objects: &O,
    log: &L,
    drafts: &S,
    repository: RepositoryId,
    draft: &DraftId,
    path: Path,
    change: Change,
) -> Result<(), StageError>
where
    O: ObjectStore,
    L: ChangesetLog,
    S: DraftStore,
{
    if let Change::File(file) = change
        && !objects.exists(repository, file.content).await?
    {
        return Err(StageError::ObjectMissing(file.content));
    }
    let base = log.read_ref(repository, Ref::Main).await?;
    drafts.stage(repository, draft, base, path, change).await?;
    Ok(())
}

/// Why a change was not staged.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum StageError {
    /// The object store holds no object with this address; it was never uploaded.
    ObjectMissing(Address),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// The draft store failed.
    Drafts(DraftStoreError),
}

impl fmt::Display for StageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ObjectMissing(address) => write!(f, "object {address} was not uploaded"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for StageError {}

impl From<ObjectStoreError> for StageError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for StageError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DraftStoreError> for StageError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}
