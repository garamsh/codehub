use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::directory_changes::{ChangeKind, RepositoryChange};
use app::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};

use crate::directory_outbox::MemoryDirectoryOutbox;
use crate::lock;

/// An outbox entry and the time its lease lapses; an entry never claimed has lapsed already.
pub(crate) struct Entry {
    pub(crate) change: RepositoryChange,
    pub(crate) lease_expires_at: i64,
}

#[derive(Default)]
pub(crate) struct Directory {
    organisations: BTreeMap<OrganisationName, BTreeMap<RepositoryName, RepositoryId>>,
    repositories: u64,
    /// The entries not yet deleted, in the order written.
    pub(crate) outbox: Vec<Entry>,
    /// The sequence of the last change written for each repository.
    sequences: BTreeMap<RepositoryId, u64>,
}

/// A repository directory in memory; it numbers repositories from 1 in creation order, and writes
/// each creation as an outbox entry under the lock the creation is made in.
#[derive(Default)]
pub struct MemoryRepositoryDirectory {
    directory: Arc<Mutex<Directory>>,
}

impl MemoryRepositoryDirectory {
    /// The outbox over the entries this directory writes.
    #[must_use]
    pub fn outbox(&self) -> MemoryDirectoryOutbox {
        MemoryDirectoryOutbox::new(Arc::clone(&self.directory))
    }

    /// Writes the entry of a further change to `repository`, with a sequence one above its last,
    /// as the directory does for a deletion once repositories can be deleted. `None` where there
    /// is no such repository.
    #[must_use]
    pub fn record_change(
        &self,
        repository: RepositoryId,
        kind: ChangeKind,
    ) -> Option<RepositoryChange> {
        let mut directory = lock(&self.directory);
        let (organisation, name) =
            directory
                .organisations
                .iter()
                .find_map(|(organisation, repositories)| {
                    repositories
                        .iter()
                        .find(|(_, id)| **id == repository)
                        .map(|(name, _)| (organisation.clone(), name.clone()))
                })?;
        let sequence = directory
            .sequences
            .get(&repository)
            .copied()
            .unwrap_or(0)
            .saturating_add(1);
        directory.sequences.insert(repository, sequence);
        let change = RepositoryChange {
            organisation,
            name,
            repository,
            kind,
            sequence,
        };
        directory.outbox.push(Entry {
            change: change.clone(),
            lease_expires_at: i64::MIN,
        });
        Some(change)
    }
}

impl RepositoryDirectory for MemoryRepositoryDirectory {
    async fn create_organisation(&self, name: &OrganisationName) -> Result<(), DirectoryError> {
        let mut directory = lock(&self.directory);
        if directory.organisations.contains_key(name) {
            return Err(DirectoryError::OrganisationExists(name.clone()));
        }
        directory
            .organisations
            .insert(name.clone(), BTreeMap::new());
        Ok(())
    }

    async fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        let mut directory = lock(&self.directory);
        let id = RepositoryId::new(directory.repositories.saturating_add(1));
        let repositories = directory
            .organisations
            .get_mut(organisation)
            .ok_or_else(|| DirectoryError::OrganisationNotFound(organisation.clone()))?;
        if repositories.contains_key(repository) {
            return Err(DirectoryError::RepositoryExists(
                organisation.clone(),
                repository.clone(),
            ));
        }
        repositories.insert(repository.clone(), id);
        directory.repositories = directory.repositories.saturating_add(1);
        directory.sequences.insert(id, 1);
        directory.outbox.push(Entry {
            change: RepositoryChange {
                organisation: organisation.clone(),
                name: repository.clone(),
                repository: id,
                kind: ChangeKind::Created,
                sequence: 1,
            },
            lease_expires_at: i64::MIN,
        });
        Ok(id)
    }

    async fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, DirectoryError> {
        lock(&self.directory)
            .organisations
            .get(organisation)
            .and_then(|repositories| repositories.get(repository))
            .copied()
            .ok_or_else(|| {
                DirectoryError::RepositoryNotFound(organisation.clone(), repository.clone())
            })
    }

    async fn names_of(
        &self,
        repository: RepositoryId,
    ) -> Result<Option<(OrganisationName, RepositoryName)>, DirectoryError> {
        Ok(lock(&self.directory)
            .organisations
            .iter()
            .find_map(|(organisation, repositories)| {
                repositories
                    .iter()
                    .find(|(_, id)| **id == repository)
                    .map(|(name, _)| (organisation.clone(), name.clone()))
            }))
    }
}
