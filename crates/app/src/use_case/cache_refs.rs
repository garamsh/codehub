//! Moving a cache ref: forward along `main`, to the tree a passing run left in its cache path
//! (ADR 0049); and the cache refs of a workspace instance: seeded from `main`'s, moved after a run
//! and removed when the instance closes (ADR 0051).

use crate::domain::address::Address;
use crate::domain::draft::Instance;
use crate::domain::job::{CacheRef, empty_output_tree};
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

/// The tree of the workspace cache ref of `path` of `instance` in `repository`, which a run
/// starts from. Where the instance has no ref for `path`, creates it from the cache ref of `path`
/// in the whole repository's folder, which is `main`'s, or empty where `main` has none. Where the
/// ref is created between the read and the create, returns the tree that was created.
///
/// # Errors
///
/// The failure of the cache refs.
pub async fn seed_workspace_cache_ref(
    refs: &dyn CacheRefs,
    repository: RepositoryId,
    instance: Instance,
    path: &Path,
) -> Result<Address, CacheRefsError> {
    if let Some(held) = refs.workspace_cache_ref(repository, instance, path).await? {
        return Ok(held);
    }
    let seed = match refs.cache_ref(repository, &Prefix::whole(), path).await? {
        Some(main) => main.tree,
        None => empty_output_tree(),
    };
    create_workspace_cache_ref(refs, repository, instance, path, seed).await
}

/// Creates the workspace cache ref of `path` of `instance` in `repository` at `seed`, for a
/// caller that has read the ref it seeds from. Where the ref is created between the read and the
/// create, returns the tree that was created.
///
/// # Errors
///
/// The failure of the cache refs.
pub async fn create_workspace_cache_ref(
    refs: &dyn CacheRefs,
    repository: RepositoryId,
    instance: Instance,
    path: &Path,
    seed: Address,
) -> Result<Address, CacheRefsError> {
    if refs
        .create_workspace_cache_ref(repository, instance, path, seed)
        .await?
    {
        return Ok(seed);
    }
    Ok(refs
        .workspace_cache_ref(repository, instance, path)
        .await?
        .unwrap_or(seed))
}

/// What a run did to one cache path of its workspace.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CacheOutcome {
    /// The cache path.
    pub path: Path,
    /// The tree the run started from.
    pub started: Address,
    /// The tree the run left.
    pub left: Address,
}

/// Moves the workspace cache ref of each of `caches` of `instance` in `repository` from the tree
/// its run started from to the tree it left. Returns the paths of the refs that did not move
/// because another run moved them meanwhile, whose newer trees stay.
///
/// # Errors
///
/// The failure of the cache refs.
pub async fn move_workspace_cache_refs(
    refs: &dyn CacheRefs,
    repository: RepositoryId,
    instance: Instance,
    caches: &[CacheOutcome],
) -> Result<Vec<Path>, CacheRefsError> {
    let mut stale = Vec::new();
    for cache in caches {
        let moved = refs
            .move_workspace_cache_ref(repository, instance, &cache.path, cache.started, cache.left)
            .await?;
        if !moved {
            stale.push(cache.path.clone());
        }
    }
    Ok(stale)
}

/// Removes the workspace cache refs of `instance` in `repository`, as its closing does.
///
/// # Errors
///
/// The failure of the cache refs.
pub async fn remove_workspace_cache_refs(
    refs: &dyn CacheRefs,
    repository: RepositoryId,
    instance: Instance,
) -> Result<(), CacheRefsError> {
    refs.remove_workspace_cache_refs(repository, instance).await
}
