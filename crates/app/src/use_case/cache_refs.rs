//! Moving a cache ref: forward along `main`, to the tree a passing run left in its cache path
//! (ADR 0049).

use crate::domain::address::Address;
use crate::domain::job::CacheRef;
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;
use crate::port::driven::cache_refs::{CacheRefs, CacheRefsError};
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError};
use crate::use_case::first_parent::advance_forward;

/// Points the cache ref of `path`, which is relative to `folder`, in `repository` at `tree`, as
/// left by `changeset`, unless it records `changeset` or a descendant of it, following first
/// parents. Where the ref moves between the read and the compare-and-swap, decides again over
/// where it moved.
///
/// # Errors
///
/// The failure of the cache refs or of the changeset log.
pub(crate) async fn advance<E>(
    refs: &dyn CacheRefs,
    log: &dyn ChangesetLog,
    repository: RepositoryId,
    folder: &Prefix,
    path: &Path,
    changeset: Address,
    tree: Address,
) -> Result<(), E>
where
    E: From<CacheRefsError> + From<ChangesetLogError>,
{
    advance_forward(
        log,
        repository,
        changeset,
        || async {
            let current = refs.cache_ref(repository, folder, path).await?;
            Ok::<_, E>(current.map(|current| current.changeset))
        },
        |current| async move {
            Ok::<_, E>(
                refs.advance_cache_ref(
                    repository,
                    folder,
                    path,
                    current,
                    CacheRef { tree, changeset },
                )
                .await?,
            )
        },
    )
    .await
}
