//! Staging: one path's change recorded in a draft, the first staging basing the draft on `main`.

use crate::domain::changeset::Change;
use crate::domain::draft::DraftId;
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::source_control::StageError;

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
