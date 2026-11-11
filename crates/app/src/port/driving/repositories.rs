//! Repositories: organisations and the repositories in them, created and looked up.

use std::fmt;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::repository::RepositoryId;
use crate::port::driven::repository_directory::DirectoryError;

/// What the outside asks of the application's organisations and repositories.
pub trait Repositories {
    /// Creates the organisation `name`.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Directory`] with [`DirectoryError::OrganisationExists`] where `name`
    /// is taken.
    fn create_organisation(
        &self,
        name: &OrganisationName,
    ) -> impl Future<Output = Result<(), RepositoriesError>> + Send;

    /// Creates the repository `repository` in `organisation` and returns its identifier.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Directory`] with [`DirectoryError::OrganisationNotFound`] where
    /// `organisation` does not exist, or [`DirectoryError::RepositoryExists`] where it already
    /// holds a repository of that name.
    fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, RepositoriesError>> + Send;

    /// The identifier of the repository `organisation`/`repository`.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Directory`] with [`DirectoryError::RepositoryNotFound`] where there is
    /// no such repository.
    fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, RepositoriesError>> + Send;
}

/// Why the organisations and repositories did not do what they were asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RepositoriesError {
    /// The directory refused or failed.
    Directory(DirectoryError),
}

impl fmt::Display for RepositoriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RepositoriesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory(error) => Some(error),
        }
    }
}

impl From<DirectoryError> for RepositoriesError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}
