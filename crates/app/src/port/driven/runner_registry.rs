//! The runner registry: the principals registered as runners.

use std::fmt;

use crate::domain::name::OrganisationName;
use crate::domain::principal::Principal;
use crate::domain::runner::{NewRegistration, Registration, RegistrationId};

/// Holds the registrations. A principal may hold several.
pub trait RunnerRegistry {
    /// Records `registration` and returns its identifier.
    ///
    /// # Errors
    ///
    /// [`RunnerRegistryError::Backend`] where the registry fails.
    fn register(
        &self,
        registration: NewRegistration,
    ) -> impl Future<Output = Result<RegistrationId, RunnerRegistryError>> + Send;

    /// Removes the registration `id` of `organisation`.
    ///
    /// # Errors
    ///
    /// [`RunnerRegistryError::NotFound`] where `organisation` has no such registration.
    fn remove(
        &self,
        organisation: &OrganisationName,
        id: RegistrationId,
    ) -> impl Future<Output = Result<(), RunnerRegistryError>> + Send;

    /// The registrations of `organisation`, oldest first.
    ///
    /// # Errors
    ///
    /// [`RunnerRegistryError::Backend`] where the registry fails.
    fn list(
        &self,
        organisation: &OrganisationName,
    ) -> impl Future<Output = Result<Vec<Registration>, RunnerRegistryError>> + Send;

    /// The registrations of `principal` in every organisation, oldest first.
    ///
    /// # Errors
    ///
    /// [`RunnerRegistryError::Backend`] where the registry fails.
    fn of_principal(
        &self,
        principal: &Principal,
    ) -> impl Future<Output = Result<Vec<Registration>, RunnerRegistryError>> + Send;
}

/// Why the runner registry refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunnerRegistryError {
    /// The organisation has no registration with this identifier.
    NotFound(RegistrationId),
    /// The registry's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for RunnerRegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no runner registration {id}"),
            Self::Backend(description) => write!(f, "runner registry failed: {description}"),
        }
    }
}

impl std::error::Error for RunnerRegistryError {}
