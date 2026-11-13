//! A path's history: its file-version nodes, derived on the first query that needs them
//! (ADR 0011).

use std::collections::BTreeMap;

use crate::domain::address::Address;
use crate::domain::derivation;
use crate::domain::file_version::{self, FileVersions};
use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driving::source_control::{HistoryError, Version};

/// The versions of `path` as of the changeset `at`, or as of `main` where `at` is `None`, newest
/// first; none where `main` is unset.
///
/// Derives and stores the file-version nodes of `at` and of each ancestor that lacks them, and reads
/// each version's message from its changeset.
///
/// # Errors
///
/// The failure of a port, or [`HistoryError::FileVersions`] where a changeset's nodes cannot be
/// derived.
pub async fn history<L: ChangesetLog, D: DerivedData>(
    log: &L,
    derived: &D,
    repository: RepositoryId,
    at: Option<Address>,
    path: &Path,
) -> Result<Vec<Version>, HistoryError> {
    let at = match at {
        Some(at) => at,
        None => match log.read_ref(repository, Ref::Main).await? {
            Some(main) => main,
            None => return Ok(Vec::new()),
        },
    };
    derive_file_versions(log, derived, repository, at).await?;
    let value = derived
        .file_versions(repository, at)
        .await?
        .ok_or(DerivedDataError::NotFound(at))?;

    let mut nodes = BTreeMap::new();
    loop {
        match file_version::history(path, &value, &nodes) {
            Ok(versions) => {
                let mut found = Vec::with_capacity(versions.len());
                for node in versions {
                    found.push(Version {
                        changeset: node.changeset,
                        path: node.path.clone(),
                        file: node.file,
                        previous: node.previous.clone(),
                        message: log.get(repository, node.changeset).await?.message,
                    });
                }
                return Ok(found);
            }
            Err(file_version::HistoryError::MissingNode(address)) => {
                let node = derived.file_version_node(repository, address).await?;
                nodes.insert(address, node);
            }
        }
    }
}

/// Derives the file-version nodes of `at`, after those of each ancestor that lacks them.
async fn derive_file_versions<L: ChangesetLog, D: DerivedData>(
    log: &L,
    derived: &D,
    repository: RepositoryId,
    at: Address,
) -> Result<(), HistoryError> {
    let mut pending = vec![at];
    while let Some(id) = pending.pop() {
        if derived.file_versions(repository, id).await?.is_some() {
            continue;
        }
        let changeset = log.get(repository, id).await?;
        let mut values = Vec::with_capacity(changeset.parents.len());
        let mut underived = Vec::new();
        for parent in &changeset.parents {
            match derived.file_versions(repository, *parent).await? {
                Some(value) => values.push(value),
                None => underived.push(*parent),
            }
        }
        if underived.is_empty() {
            let step = derivation::derive(&FileVersions, &changeset, &values)?;
            derived
                .put_file_versions(repository, id, step.value, step.objects)
                .await?;
        } else {
            // Revisited once its parents are derived.
            pending.push(id);
            pending.extend(underived);
        }
    }
    Ok(())
}
