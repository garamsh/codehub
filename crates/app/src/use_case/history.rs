//! A path's history: its file-version nodes, derived on the first query that needs them
//! (ADR 0011).

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::derivation::{self, DerivationError};
use crate::domain::file_version::{self, FileVersionError, FileVersionNode, FileVersions};
use crate::domain::path::Path;
use crate::port::driven::changeset_log::{ChangesetLog, ChangesetLogError};
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};

/// The versions of `path` as of the changeset `at`, newest first.
///
/// Derives and stores the file-version nodes of `at` and of each ancestor that lacks them.
///
/// # Errors
///
/// The failure of a port, or [`HistoryError::FileVersions`] where a changeset's nodes cannot be
/// derived.
pub async fn history<L: ChangesetLog, D: DerivedData>(
    log: &L,
    derived: &D,
    at: Address,
    path: &Path,
) -> Result<Vec<FileVersionNode>, HistoryError> {
    derive_file_versions(log, derived, at).await?;
    let value = derived
        .file_versions(at)
        .await?
        .ok_or(DerivedDataError::NotFound(at))?;

    let mut nodes = BTreeMap::new();
    loop {
        match file_version::history(path, &value, &nodes) {
            Ok(versions) => return Ok(versions.into_iter().cloned().collect()),
            Err(file_version::HistoryError::MissingNode(address)) => {
                let node = derived.file_version_node(address).await?;
                nodes.insert(address, node);
            }
        }
    }
}

/// Derives the file-version nodes of `at`, after those of each ancestor that lacks them.
async fn derive_file_versions<L: ChangesetLog, D: DerivedData>(
    log: &L,
    derived: &D,
    at: Address,
) -> Result<(), HistoryError> {
    let mut pending = vec![at];
    while let Some(id) = pending.pop() {
        if derived.file_versions(id).await?.is_some() {
            continue;
        }
        let changeset = log.get(id).await?;
        let mut values = Vec::with_capacity(changeset.parents.len());
        let mut underived = Vec::new();
        for parent in &changeset.parents {
            match derived.file_versions(*parent).await? {
                Some(value) => values.push(value),
                None => underived.push(*parent),
            }
        }
        if underived.is_empty() {
            let step = derivation::derive(&FileVersions, &changeset, &values)?;
            derived
                .put_file_versions(id, step.value, step.objects)
                .await?;
        } else {
            // Revisited once its parents are derived.
            pending.push(id);
            pending.extend(underived);
        }
    }
    Ok(())
}

/// Why a history could not be read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum HistoryError {
    /// A changeset's file-version nodes could not be derived.
    FileVersions(DerivationError<FileVersionError>),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
}

impl fmt::Display for HistoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FileVersions(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for HistoryError {}

impl From<DerivationError<FileVersionError>> for HistoryError {
    fn from(error: DerivationError<FileVersionError>) -> Self {
        Self::FileVersions(error)
    }
}

impl From<ChangesetLogError> for HistoryError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for HistoryError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}
