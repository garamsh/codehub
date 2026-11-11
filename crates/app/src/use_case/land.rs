//! Landing: a draft rebased onto the current `main`, its tree derived, appended to the log, and
//! `main` advanced to it by compare-and-swap (ADR 0012).

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::{Change, Changeset};
use crate::domain::derivation::{self, DerivationError, Step};
use crate::domain::encoding::EncodeError;
use crate::domain::file::File;
use crate::domain::merge::{self, FileConflict, FileVersion, Rebase};
use crate::domain::path::Path;
use crate::domain::tree::{DeriveError, Entry, Tree, Trees};
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::draft_store::{DraftStore, DraftStoreError};
use crate::port::driven::object_store::{ObjectStore, ObjectStoreError};

/// Lands the draft stored under `draft` on `main` and returns the landed changeset's identifier.
///
/// The draft's delta is rebased onto `main` as it is read; its first parent becomes `main`, and
/// `main` advances to it only where it still points where it was read.
///
/// # Errors
///
/// [`LandError::Conflicts`] where the draft conflicts with what `main` changed since its base,
/// [`LandError::MainMoved`] where `main` moved before the compare-and-swap, and otherwise the
/// failure of a port or of the tree derivation. Nothing moves `main` on an error.
pub async fn land<O, L, D, S>(
    objects: &O,
    log: &L,
    derived: &D,
    drafts: &S,
    draft: Address,
) -> Result<Address, LandError>
where
    O: ObjectStore,
    L: ChangesetLog,
    D: DerivedData,
    S: DraftStore,
{
    let draft = drafts.get(draft).await?;
    let main = log.read_ref(Ref::Main).await?;
    let base = draft.parents.first().copied();
    let parents = main
        .into_iter()
        .chain(draft.parents.iter().skip(1).copied())
        .collect();
    let changes = rebase(objects, log, derived, base, main, draft.changes).await?;
    let changeset = Changeset {
        parents,
        author: draft.author,
        timestamp: draft.timestamp,
        message: draft.message,
        changes,
    };

    let trees = derive_trees(derived, &changeset).await?;
    let id = changeset.id()?;
    derived.put_trees(id, trees.value, trees.objects).await?;
    log.append(changeset).await?;
    log.compare_and_swap(Ref::Main, main, id)
        .await
        .map_err(|error| match error {
            ChangesetLogError::RefMoved { current } => LandError::MainMoved { current },
            ChangesetLogError::NotFound(_) | ChangesetLogError::Encode(_) => LandError::Log(error),
        })?;
    Ok(id)
}

/// The landing's delta against `main`: unchanged where no path overlaps what `main` changed since
/// `base`, and merged where paths overlap.
async fn rebase<O: ObjectStore, L: ChangesetLog, D: DerivedData>(
    objects: &O,
    log: &L,
    derived: &D,
    base: Option<Address>,
    main: Option<Address>,
    landing: BTreeMap<Path, Change>,
) -> Result<BTreeMap<Path, Change>, LandError> {
    let since = delta_since(log, base, main).await?;
    let root = match base {
        Some(base) => Some(derived.root(base).await?),
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
        if let Some(file) = file_at(derived, root, path).await? {
            base_versions.insert(path.clone(), version(objects, file).await?);
        }
        ours.insert(path.clone(), merge_change(objects, *main_change).await?);
        theirs.insert(path.clone(), merge_change(objects, *change).await?);
    }

    match merge::rebase(&base_versions, &ours, &theirs) {
        Ok(Rebase::Reparent) => Ok(landing),
        Ok(Rebase::Merge(merged)) => {
            let mut changes = landing;
            changes.retain(|path, _| !ours.contains_key(path));
            for (path, change) in merged {
                changes.insert(path, stored(objects, change).await?);
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
    base: Option<Address>,
    main: Option<Address>,
) -> Result<BTreeMap<Path, Change>, LandError> {
    let mut deltas = Vec::new();
    let mut at = main;
    while at != base {
        let Some(id) = at else {
            return Err(LandError::BaseNotOnMain);
        };
        let changeset = log.get(id).await?;
        at = changeset.parents.first().copied();
        deltas.push(changeset.changes);
    }
    let mut delta = BTreeMap::new();
    for changes in deltas.into_iter().rev() {
        delta.extend(changes);
    }
    Ok(delta)
}

/// The file at `path` in the tree rooted at `root`, if it holds one.
async fn file_at<D: DerivedData>(
    derived: &D,
    root: Option<Address>,
    path: &Path,
) -> Result<Option<File>, LandError> {
    let Some(mut tree) = root else {
        return Ok(None);
    };
    let Some((name, directories)) = path.components().split_last() else {
        return Ok(None);
    };
    for directory in directories {
        match derived.tree(tree).await?.entries.get(directory) {
            Some(Entry::Directory(subtree)) => tree = *subtree,
            Some(Entry::File(_)) | None => return Ok(None),
        }
    }
    match derived.tree(tree).await?.entries.get(name) {
        Some(Entry::File(file)) => Ok(Some(*file)),
        Some(Entry::Directory(_)) | None => Ok(None),
    }
}

async fn version<O: ObjectStore>(objects: &O, file: File) -> Result<FileVersion, LandError> {
    Ok(FileVersion {
        kind: file.kind,
        content: objects.get(file.content).await?,
    })
}

async fn merge_change<O: ObjectStore>(
    objects: &O,
    change: Change,
) -> Result<merge::Change, LandError> {
    Ok(match change {
        Change::Deletion => merge::Change::Delete,
        Change::File(file) => merge::Change::Write(version(objects, file).await?),
    })
}

/// A merged change as a delta records it, its content stored in the object store.
async fn stored<O: ObjectStore>(objects: &O, change: merge::Change) -> Result<Change, LandError> {
    Ok(match change {
        merge::Change::Delete => Change::Deletion,
        merge::Change::Write(version) => Change::File(File {
            kind: version.kind,
            content: objects.put(version.content).await?,
        }),
    })
}

/// Derives `changeset`'s trees, loading each tree the derivation asks for.
async fn derive_trees<D: DerivedData>(
    derived: &D,
    changeset: &Changeset,
) -> Result<Step<Address, Tree>, LandError> {
    let mut roots = Vec::with_capacity(changeset.parents.len());
    for parent in &changeset.parents {
        roots.push(derived.root(*parent).await?);
    }
    let mut known = BTreeMap::new();
    loop {
        match derivation::derive(&Trees { known: &known }, changeset, &roots) {
            Ok(step) => return Ok(step),
            Err(DerivationError::Step(DeriveError::MissingTree(address))) => {
                let tree = derived.tree(address).await?;
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

/// Why a path conflicts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathConflict {
    /// The two sides' versions of the file do not merge.
    File(FileConflict),
    /// The path holds a file where the other side has a directory, or lies under one.
    FileAgainstDirectory,
}

/// Why a draft did not land.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LandError {
    /// Paths conflict with what `main` changed since the draft's base; each with its reason.
    Conflicts(BTreeMap<Path, PathConflict>),
    /// `main` moved between the landing's read and its compare-and-swap; the landing was not
    /// applied.
    MainMoved {
        /// Where `main` points.
        current: Option<Address>,
    },
    /// The draft's base is not `main` or a first-parent ancestor of it.
    BaseNotOnMain,
    /// The draft's tree could not be derived.
    Tree(DerivationError<DeriveError>),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The draft store failed.
    Drafts(DraftStoreError),
    /// The landed changeset has no encoding.
    Encode(EncodeError),
}

impl fmt::Display for LandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflicts(paths) => write!(f, "{} conflicting paths", paths.len()),
            Self::MainMoved {
                current: Some(current),
            } => write!(f, "main moved to {current}"),
            Self::MainMoved { current: None } => write!(f, "main was unset"),
            Self::BaseNotOnMain => write!(f, "the draft's base is not on main"),
            Self::Tree(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
            Self::Encode(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for LandError {}

impl From<ObjectStoreError> for LandError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for LandError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for LandError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<DraftStoreError> for LandError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

impl From<EncodeError> for LandError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}
