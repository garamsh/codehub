//! The repositories driving port, served by the repository directory it is given.

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::repository::RepositoryId;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::repositories::{Repositories, RepositoriesError};

/// Organisations and repositories kept in the directory the composition site passes to it.
pub struct RepositoriesService<'a, R> {
    directory: &'a R,
}

impl<'a, R> RepositoriesService<'a, R> {
    /// Repositories kept in `directory`.
    pub const fn new(directory: &'a R) -> Self {
        Self { directory }
    }
}

impl<R: RepositoryDirectory + Sync> Repositories for RepositoriesService<'_, R> {
    async fn create_organisation(&self, name: &OrganisationName) -> Result<(), RepositoriesError> {
        Ok(self.directory.create_organisation(name).await?)
    }

    async fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        Ok(self
            .directory
            .create_repository(organisation, repository)
            .await?)
    }

    async fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        Ok(self
            .directory
            .find_repository(organisation, repository)
            .await?)
    }
}
