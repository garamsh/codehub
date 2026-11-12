//! Runners: the principals an organisation registers to run its checks.

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::name::{Label, OrganisationName, RepositoryName};
use crate::domain::principal::{Caller, Principal};
use crate::domain::runner::{Registration, RegistrationId};
use crate::port::driven::repository_directory::DirectoryError;
use crate::port::driven::runner_registry::RunnerRegistryError;
use crate::port::driving::access::AccessError;

/// What the outside asks of an organisation's runners. Each operation needs the caller to be
/// allowed to administer the organisation's runners.
pub trait Runners {
    /// Registers `principal` as a runner of `organisation` offering `labels`, for the checks of
    /// `repository` or, where it is `None`, of every repository of the organisation. Returns the
    /// registration's identifier.
    ///
    /// # Errors
    ///
    /// [`RunnersError::Access`] where `caller` may not administer the runners of `organisation`,
    /// and [`RunnersError::Directory`] with [`DirectoryError::RepositoryNotFound`] where
    /// `organisation` holds no such repository.
    fn register(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: Option<&RepositoryName>,
        principal: &Principal,
        labels: BTreeSet<Label>,
    ) -> impl Future<Output = Result<RegistrationId, RunnersError>> + Send;

    /// Removes the registration `id` of `organisation`.
    ///
    /// # Errors
    ///
    /// [`RunnersError::Access`] where `caller` may not administer the runners of `organisation`,
    /// and [`RunnersError::Registry`] with [`RunnerRegistryError::NotFound`] where it has no such
    /// registration.
    fn remove(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        id: RegistrationId,
    ) -> impl Future<Output = Result<(), RunnersError>> + Send;

    /// The registrations of `organisation`, oldest first.
    ///
    /// # Errors
    ///
    /// [`RunnersError::Access`] where `caller` may not administer the runners of `organisation`.
    fn list(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
    ) -> impl Future<Output = Result<Vec<Registration>, RunnersError>> + Send;
}

/// Why the runners were not changed or listed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunnersError {
    /// The caller may not administer the organisation's runners.
    Access(AccessError),
    /// The repository directory refused or failed.
    Directory(DirectoryError),
    /// The runner registry refused or failed.
    Registry(RunnerRegistryError),
}

impl fmt::Display for RunnersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Directory(error) => write!(f, "{error}"),
            Self::Registry(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RunnersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Directory(error) => Some(error),
            Self::Registry(error) => Some(error),
        }
    }
}

impl From<AccessError> for RunnersError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<DirectoryError> for RunnersError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}

impl From<RunnerRegistryError> for RunnersError {
    fn from(error: RunnerRegistryError) -> Self {
        Self::Registry(error)
    }
}
