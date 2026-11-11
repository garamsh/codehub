//! Repositories: organisations and the repositories in them, created and looked up.

use std::fmt;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::membership::MembershipError;
use crate::port::driven::repository_directory::DirectoryError;
use crate::use_case::access::AccessError;

/// What the outside asks of the application's organisations and repositories.
pub trait Repositories {
    /// Creates the organisation `name`, with `caller` as its admin.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Directory`] with [`DirectoryError::OrganisationExists`] where `name`
    /// is taken.
    fn create_organisation(
        &self,
        caller: &Caller,
        name: &OrganisationName,
    ) -> impl Future<Output = Result<(), RepositoriesError>> + Send;

    /// Creates the repository `repository` in `organisation` and returns its identifier.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Access`] where `caller` is not an admin of `organisation`, which
    /// holds for an organisation that does not exist; [`RepositoriesError::Directory`] with
    /// [`DirectoryError::RepositoryExists`] where it already holds a repository of that name.
    fn create_repository(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, RepositoriesError>> + Send;

    /// The identifier of the repository `organisation`/`repository`.
    ///
    /// # Errors
    ///
    /// [`RepositoriesError::Directory`] with [`DirectoryError::RepositoryNotFound`] where there is
    /// no such repository, or `caller` holds no read on it.
    fn find_repository(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> impl Future<Output = Result<RepositoryId, RepositoriesError>> + Send;
}

/// Why the organisations and repositories did not do what they were asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RepositoriesError {
    /// The caller may not do it.
    Access(AccessError),
    /// The directory refused or failed.
    Directory(DirectoryError),
    /// Membership failed while the creator was made admin.
    Membership(MembershipError),
}

impl fmt::Display for RepositoriesError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Directory(error) => write!(f, "{error}"),
            Self::Membership(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RepositoriesError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Directory(error) => Some(error),
            Self::Membership(error) => Some(error),
        }
    }
}

impl From<AccessError> for RepositoriesError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<DirectoryError> for RepositoriesError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}

impl From<MembershipError> for RepositoriesError {
    fn from(error: MembershipError) -> Self {
        Self::Membership(error)
    }
}
