//! The repository directory: the organisations and the repositories in them, and the identifier
//! each repository is known by inside the core.

use std::fmt;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::repository::RepositoryId;

/// Holds the organisations and their repositories.
pub trait RepositoryDirectory {
    /// Creates the organisation `name`.
    ///
    /// # Errors
    ///
    /// [`DirectoryError::OrganisationExists`] where `name` is taken.
    fn create_organisation(
        &self,
        name: &OrganisationName,
    ) -> impl Future<Output = Result<(), DirectoryError>> + Send;

    /// Creates the repository `repository` in `organisation` and returns its identifier.
    ///
    /// # Errors
    ///
    /// [`DirectoryError::OrganisationNotFound`] where `organisation` does not exist, and
    /// [`DirectoryError::RepositoryExists`] where it already holds a repository of that name.
    fn create_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, DirectoryError>> + Send;

    /// The identifier of the repository `organisation`/`repository`.
    ///
    /// # Errors
    ///
    /// [`DirectoryError::RepositoryNotFound`] where there is no such repository.
    fn find_repository(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, DirectoryError>> + Send;
}

/// Why the directory refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DirectoryError {
    /// An organisation of this name exists.
    OrganisationExists(OrganisationName),
    /// No organisation has this name.
    OrganisationNotFound(OrganisationName),
    /// The organisation holds a repository of this name.
    RepositoryExists(OrganisationName, RepositoryName),
    /// The organisation holds no repository of this name.
    RepositoryNotFound(OrganisationName, RepositoryName),
}

impl fmt::Display for DirectoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OrganisationExists(name) => write!(f, "organisation {name} exists"),
            Self::OrganisationNotFound(name) => write!(f, "no organisation {name}"),
            Self::RepositoryExists(organisation, name) => {
                write!(f, "repository {organisation}/{name} exists")
            }
            Self::RepositoryNotFound(organisation, name) => {
                write!(f, "no repository {organisation}/{name}")
            }
        }
    }
}

impl std::error::Error for DirectoryError {}
