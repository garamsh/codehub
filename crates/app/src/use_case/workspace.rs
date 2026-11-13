//! The workspace: a draft read against its base, as its changes with their status and as a diff
//! (ADR 0048).

use std::collections::BTreeMap;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::diff::{self, Side};
use crate::domain::draft::{ChangeStatus, Draft, Edit};
use crate::domain::file::{File, FileKind};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::source_control::{ChangedPath, PathDiff, Workspace, WorkspaceError};
use crate::use_case::read_file::file_at;

/// One path a draft changes against its base.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Changed<'a> {
    pub(crate) path: &'a Path,
    pub(crate) status: ChangeStatus,
    /// The base's file at the path, if it holds one.
    pub(crate) before: Option<File>,
    pub(crate) change: Change,
}

impl Changed<'_> {
    /// The file mode the path has in the draft, or in the base where the draft deletes it.
    fn kind(&self) -> FileKind {
        match self.change {
            Change::File(file) => file.kind,
            Change::Deletion => self.before.map_or(FileKind::Regular, |file| file.kind),
        }
    }
}

/// The paths `draft` changes, in path order, each with its status against the draft's base. A path
/// whose change leaves the base's file as it is is left out.
pub(crate) async fn changed<'a, D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    draft: &'a Draft,
) -> Result<Vec<Changed<'a>>, DerivedDataError> {
    let root = match draft.base {
        Some(base) => Some(derived.root(repository, base).await?),
        None => None,
    };
    let mut changed = Vec::new();
    for (path, change) in &draft.changes {
        let before = file_at(derived, repository, root, path).await?;
        if let Some(status) = ChangeStatus::of(before, *change) {
            changed.push(Changed {
                path,
                status,
                before,
                change: *change,
            });
        }
    }
    Ok(changed)
}

/// `draft` as a workspace listing `changed`, each added or modified path with a grant to download
/// its content.
pub(crate) async fn workspace<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    draft: &Draft,
    changed: &[Changed<'_>],
) -> Result<Workspace, WorkspaceError> {
    let mut changes = Vec::new();
    for one in changed {
        let download = match one.change {
            Change::File(file) => Some(objects.download_grant(repository, file.content).await?),
            Change::Deletion => None,
        };
        changes.push(ChangedPath {
            path: one.path.clone(),
            status: one.status,
            kind: one.kind(),
            download,
        });
    }
    Ok(Workspace {
        base: draft.base,
        description: draft.description.clone(),
        changes,
    })
}

/// The difference from the base to the draft of each of `changed`.
pub(crate) async fn diffs<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    changed: &[Changed<'_>],
) -> Result<Vec<PathDiff>, WorkspaceError> {
    let mut diffs = Vec::new();
    for one in changed {
        let before = content(objects, repository, one.before).await?;
        let after = match one.change {
            Change::File(file) => content(objects, repository, Some(file)).await?,
            Change::Deletion => None,
        };
        diffs.push(PathDiff {
            path: one.path.clone(),
            status: one.status,
            diff: diff::diff(one.path, side(&before), side(&after)),
            kind_change: diff::kind_change(side(&before), side(&after)),
        });
    }
    Ok(diffs)
}

async fn content<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    file: Option<File>,
) -> Result<Option<(FileKind, Vec<u8>)>, WorkspaceError> {
    match file {
        Some(file) => Ok(Some((
            file.kind,
            objects.get(repository, file.content).await?,
        ))),
        None => Ok(None),
    }
}

fn side(content: &Option<(FileKind, Vec<u8>)>) -> Side<'_> {
    content
        .as_ref()
        .map(|(kind, bytes)| (*kind, bytes.as_slice()))
}

/// Requires that every file `edits` records is in `repository`'s object store, so that a snapshot
/// applies whole or not at all.
pub(crate) async fn require_uploaded<O: ObjectStore>(
    objects: &O,
    repository: RepositoryId,
    edits: &BTreeMap<Path, Edit>,
) -> Result<(), WorkspaceError> {
    let contents: Vec<Address> = edits
        .values()
        .filter_map(|edit| match edit {
            Edit::Record(Change::File(file)) => Some(file.content),
            Edit::Record(Change::Deletion) | Edit::Restore => None,
        })
        .collect();
    for content in contents {
        if !objects.exists(repository, content).await? {
            return Err(WorkspaceError::ObjectMissing(content));
        }
    }
    Ok(())
}
