use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::name::{OrganisationName, RepositoryName};
use app::domain::repository::RepositoryId;
use app::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};

use crate::lock;

#[derive(Default)]
struct Directory {
    organisations: BTreeMap<OrganisationName, BTreeMap<RepositoryName, RepositoryId>>,
    repositories: u64,
}

/// A repository directory in memory; it numbers repositories from 1 in creation order.
#[derive(Default)]
pub struct MemoryRepositoryDirectory {
    directory: Mutex<Directory>,
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
