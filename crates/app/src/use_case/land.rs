//! Landing: a staged draft rebased onto the current `main`, its tree derived, appended to the log, and
//! `main` advanced to it by compare-and-swap (ADR 0012).

use std::collections::BTreeMap;
use std::fmt::Display;

use crate::domain::address::Address;
use crate::domain::changeset::{Change, Changeset};
use crate::domain::derivation::{self, DerivationError, Step};
use crate::domain::draft::Draft;
use crate::domain::file::File;
use crate::domain::merge::{self, FileVersion, Rebase};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::domain::tree::{DeriveError, Tree, Trees};
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::source_control::{LandError, Landing, PathConflict};
use crate::use_case::read_file::file_at;

/// Lands the staged draft `staged` of `repository` on its `main` and returns the landed
/// changeset's identifier. The changeset's author is the text of `author`: the draft's principal,
/// or the server.
///
/// The draft's delta is rebased onto `main` as it is read; its parent becomes `main`, and `main`
/// advances to it only where it still points where it was read.
///
/// # Errors
///
/// [`LandError::Conflicts`] where the draft conflicts with what `main` changed since its base,
/// [`LandError::MainMoved`] where `main` moved before the compare-and-swap, and otherwise the
/// failure of a port or of the tree derivation. Nothing moves `main` on an error.
pub async fn land<O, L, D>(
    objects: &O,
    log: &L,
    derived: &D,
    repository: RepositoryId,
    author: &(impl Display + ?Sized),
    staged: Draft,
    landing: Landing,
) -> Result<Address, LandError>
where
    O: ObjectStore,
    L: ChangesetLog,
    D: DerivedData,
{
    let main = log.read_ref(repository, Ref::Main).await?;
    let changes = rebase(
        objects,
        log,
        derived,
        repository,
        staged.base,
        main,
        staged.changes,
    )
    .await?;
    let changeset = Changeset {
        parents: main.into_iter().collect(),
        author: author.to_string(),
        timestamp: landing.timestamp,
        message: landing.message,
        changes,
    };

    let trees = derive_trees(derived, repository, &changeset).await?;
    let id = changeset.id()?;
    derived
        .put_trees(repository, id, trees.value, trees.objects)
        .await?;
    log.append(repository, changeset).await?;
    log.compare_and_swap(repository, Ref::Main, main, id)
        .await
        .map_err(|error| match error {
            ChangesetLogError::RefMoved { current } => LandError::MainMoved { current },
            ChangesetLogError::NotFound(_)
            | ChangesetLogError::Encode(_)
            | ChangesetLogError::Backend(_) => LandError::Log(error),
        })?;
    Ok(id)
}

/// The landing's delta against `main`: unchanged where no path overlaps what `main` changed since
/// `base`, and merged where paths overlap.
async fn rebase<O: ObjectStore, L: ChangesetLog, D: DerivedData>(
    objects: &O,
    log: &L,
    derived: &D,
    repository: RepositoryId,
    base: Option<Address>,
    main: Option<Address>,
    landing: BTreeMap<Path, Change>,
) -> Result<BTreeMap<Path, Change>, LandError> {
    let since = delta_since(log, repository, base, main).await?;
    let root = match base {
        Some(base) => Some(derived.root(repository, base).await?),
        None => None,
    };

    // Blobs are loaded only for the paths both deltas change.
    let mut base_versions = BTreeMap::new();
    let mut ours = BTreeMap::new();
    let mut theirs = BTreeMap::new();
    for (path, change) in &landing {
        let Some(main_change) = since.get(path) else {
            continue;
        };
        if let Some(file) = file_at(derived, repository, root, path).await? {
            base_versions.insert(path.clone(), version(objects, repository, file).await?);
        }
        ours.insert(
            path.clone(),
            merge_change(objects, repository, *main_change).await?,
        );
        theirs.insert(
            path.clone(),
            merge_change(objects, repository, *change).await?,
        );
    }

    match merge::rebase(&base_versions, &ours, &theirs) {
        Ok(Rebase::Reparent) => Ok(landing),
        Ok(Rebase::Merge(merged)) => {
            let mut changes = landing;
            changes.retain(|path, _| !ours.contains_key(path));
            for (path, change) in merged {
                changes.insert(path, stored(objects, repository, change).await?);
            }
            Ok(changes)
        }
        Err(conflicts) => Err(LandError::Conflicts(
            conflicts
                .paths()
                .iter()
                .map(|(path, conflict)| (path.clone(), PathConflict::File(*conflict)))
                .collect(),
        )),
    }
}

/// What the changesets from `base`, exclusive, to `main`, inclusive, changed, following first
/// parents; a later change to a path replaces an earlier one.
async fn delta_since<L: ChangesetLog>(
    log: &L,
    repository: RepositoryId,
    base: Option<Address>,
    main: Option<Address>,
) -> Result<BTreeMap<Path, Change>, LandError> {
    let mut deltas = Vec::new();
    let mut at = main;
    while at != base {
        let Some(id) = at else {
            return Err(LandError::BaseNotOnMain);
        };
        let changeset = log.get(repository, id).await?;
        at = changeset.parents.first().copied();
        deltas.push(changeset.changes);
    }
    let mut delta = BTreeMap::new();
    for changes in deltas.into_iter().rev() {
        delta.extend(changes);
    }
    Ok(delta)
}

async fn version<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    file: File,
) -> Result<FileVersion, LandError> {
    Ok(FileVersion {
        kind: file.kind,
        content: objects.get(repository, file.content).await?,
    })
}

async fn merge_change<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    change: Change,
) -> Result<merge::Change, LandError> {
    Ok(match change {
        Change::Deletion => merge::Change::Delete,
        Change::File(file) => merge::Change::Write(version(objects, repository, file).await?),
    })
}

/// A merged change as a delta records it, its content stored in the object store.
async fn stored<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    change: merge::Change,
) -> Result<Change, LandError> {
    Ok(match change {
        merge::Change::Delete => Change::Deletion,
        merge::Change::Write(version) => Change::File(File {
            kind: version.kind,
            content: objects.put(repository, version.content).await?,
        }),
    })
}

/// Derives `changeset`'s trees, loading each tree the derivation asks for.
async fn derive_trees<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    changeset: &Changeset,
) -> Result<Step<Address, Tree>, LandError> {
    let mut roots = Vec::with_capacity(changeset.parents.len());
    for parent in &changeset.parents {
        roots.push(derived.root(repository, *parent).await?);
    }
    let mut known = BTreeMap::new();
    loop {
        match derivation::derive(&Trees { known: &known }, changeset, &roots) {
            Ok(step) => return Ok(step),
            Err(DerivationError::Step(DeriveError::MissingTree(address))) => {
                let tree = derived.tree(repository, address).await?;
                known.insert(address, tree);
            }
            // ADR 0019: a file left where a directory is, or under a file, conflicts on the path
            // holding the file.
            Err(DerivationError::Step(
                DeriveError::IsADirectory(path) | DeriveError::NotADirectory(path),
            )) => {
                return Err(LandError::Conflicts(BTreeMap::from([(
                    path,
                    PathConflict::FileAgainstDirectory,
                )])));
            }
            Err(
                error @ (DerivationError::ParentCount { .. }
                | DerivationError::Step(DeriveError::NotFound(_) | DeriveError::Encode(_))),
            ) => return Err(LandError::Tree(error)),
        }
    }
}
