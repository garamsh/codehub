//! The checks a tree defines: its files walked once, its check files found and read, and each
//! check keyed from its inputs and the results of the checks it needs (ADR 0049). The plan of a
//! draft and the queueing of a landing both read and key through here, so they compute one key.

use std::collections::BTreeMap;
use std::future::Future;

use crate::domain::address::Address;
use crate::domain::check::{CheckDefinition, CheckKey, needs_fault};
use crate::domain::file::File;
use crate::domain::job::{Job, Needs};
use crate::domain::path::{Path, PathError, Prefix};
use crate::domain::repository::RepositoryId;
use crate::domain::tree::{Entry, Tree};
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::object_store::{ObjectStore, ObjectStoreError};

/// Every file of the tree at `root`, by its path from the root; where `only` is a check, the files
/// it reads, reading no tree its inputs cannot reach. `read` returns the tree stored under an
/// address, and `invalid` turns a path no file can have into the caller's error.
pub(crate) async fn walk_files<E, R, F>(
    root: Address,
    read: R,
    invalid: fn(PathError) -> E,
    only: Option<&CheckDefinition>,
) -> Result<BTreeMap<Path, File>, E>
where
    R: Fn(Address) -> F,
    F: Future<Output = Result<Tree, E>>,
{
    let mut files = BTreeMap::new();
    let mut pending = vec![(Vec::new(), root)];
    while let Some((directory, address)) = pending.pop() {
        for (name, entry) in read(address).await?.entries {
            let mut components = directory.clone();
            components.push(name);
            match entry {
                Entry::Directory(subtree) => {
                    if only.is_none_or(|check| check.may_read_below(&components)) {
                        pending.push((components, subtree));
                    }
                }
                Entry::File(file) => {
                    let path = Path::new(components).map_err(invalid)?;
                    if only.is_none_or(|check| check.reads(&path)) {
                        files.insert(path, file);
                    }
                }
            }
        }
    }
    Ok(files)
}

/// The content address of every file of the changeset tree at `root`, by path.
///
/// # Errors
///
/// The failure of derived data.
pub(crate) async fn tree_files<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    root: Address,
) -> Result<BTreeMap<Path, Address>, DerivedDataError> {
    let files = walk_files(
        root,
        |address| derived.tree(repository, address),
        |error| DerivedDataError::Backend(error.to_string()),
        None,
    )
    .await?;
    Ok(files
        .into_iter()
        .map(|(path, file)| (path, file.content))
        .collect())
}

/// What a check depends on (ADR 0049): the files its inputs select in the changeset, and the key
/// and output tree of each check it needs. Its key covers it, and a claim grants it.
#[derive(Debug)]
pub(crate) struct Dependencies {
    /// The files the check's input patterns select, by path.
    pub inputs: BTreeMap<Path, File>,
    /// The key and output tree of each check it needs, in the order of its `needs`.
    pub needed: Vec<(CheckKey, Address)>,
}

impl Dependencies {
    /// The key of `check`, the check these are the dependencies of. The key covers the content of
    /// each input and not its kind.
    pub(crate) fn key(&self, check: &CheckDefinition) -> CheckKey {
        let inputs: Vec<(Path, Address)> = self
            .inputs
            .iter()
            .map(|(path, file)| (path.clone(), file.content))
            .collect();
        CheckKey::new(check, &inputs, &self.needed)
    }
}

/// Where the dependencies of a check stand.
#[derive(Debug)]
pub(crate) enum Resolved {
    /// A check it needs has not ended.
    Waiting,
    /// A check it needs did not pass, so it is not run.
    Skipped,
    /// Every check it needs passed.
    Ready(Dependencies),
}

/// The dependencies of `check` in the changeset at `root`, among `jobs`, the jobs of the
/// changeset: the one place that decides them, for the key of a queued job and for the grant of a
/// claimed one. Only the trees `check`'s inputs can reach are read.
///
/// # Errors
///
/// The failure of derived data.
pub(crate) async fn dependencies<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    root: Address,
    check: &CheckDefinition,
    jobs: &[Job],
) -> Result<Resolved, DerivedDataError> {
    let needed = match Needs::of(check, jobs) {
        Needs::Waiting => return Ok(Resolved::Waiting),
        Needs::Skipped => return Ok(Resolved::Skipped),
        Needs::Passed(needed) => needed,
    };
    let inputs = walk_files(
        root,
        |address| derived.tree(repository, address),
        |error| DerivedDataError::Backend(error.to_string()),
        Some(check),
    )
    .await?;
    Ok(Resolved::Ready(Dependencies { inputs, needed }))
}

/// The check definitions of a tree could not be read.
#[derive(Debug)]
pub(crate) enum DefinitionsError {
    /// The object store failed.
    Objects(ObjectStoreError),
    /// A check file cannot be parsed, or the checks name each other wrongly.
    Unreadable(Unreadable),
}

impl From<ObjectStoreError> for DefinitionsError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

/// The folder whose check definitions are at fault, and what is wrong with them.
#[derive(Debug)]
pub(crate) struct Unreadable {
    pub folder: Prefix,
    pub reason: String,
}

/// The checks the tree `files` defines, read from the check file of every folder, a folder before
/// the folders under it.
///
/// # Errors
///
/// [`DefinitionsError::Objects`] where a check file cannot be fetched, and
/// [`DefinitionsError::Unreadable`] where one cannot be parsed or the checks name each other
/// wrongly.
pub(crate) async fn read_definitions<O, C>(
    objects: &O,
    reader: &C,
    repository: RepositoryId,
    files: &BTreeMap<Path, Address>,
) -> Result<Vec<CheckDefinition>, DefinitionsError>
where
    O: ObjectStore,
    C: CheckDefinitions,
{
    let file = reader.file();
    let mut found: BTreeMap<Prefix, Address> = BTreeMap::new();
    for (path, content) in files {
        if let Some(folder) = path.components().strip_suffix(file.components()) {
            found.insert(Prefix::new(folder.to_vec()), *content);
        }
    }
    let mut definitions = Vec::new();
    for (folder, content) in found {
        let bytes = objects.get(repository, content).await?;
        match reader.parse(&folder, &bytes) {
            Ok(read) => definitions.extend(read),
            Err(error) => {
                return Err(DefinitionsError::Unreadable(Unreadable {
                    folder,
                    reason: error.to_string(),
                }));
            }
        }
    }
    if let Some(fault) = needs_fault(&definitions) {
        return Err(DefinitionsError::Unreadable(Unreadable {
            folder: fault.check().folder.clone(),
            reason: fault.to_string(),
        }));
    }
    Ok(definitions)
}
