//! The runners driving port, served by the runner registry, the repository directory and the
//! authorisation decision it is given.

use std::collections::BTreeSet;

use crate::domain::name::{Label, OrganisationName, RepositoryName};
use crate::domain::principal::{Caller, Principal};
use crate::domain::runner::{NewRegistration, Registration, RegistrationId};
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driven::runner_registry::RunnerRegistry;
use crate::port::driving::runners::{Runners, RunnersError};
use crate::use_case::access::require;

/// The driven ports the runners are built over.
pub struct RunnersAdapters<'a, G, R, A> {
    /// Keeps the registrations.
    pub registry: &'a G,
    /// Looks repositories up.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
}

/// Runners kept in the registry the composition site passes to it, administered by the callers
/// the authorisation decision allows.
pub struct RunnersService<'a, G, R, A> {
    registry: &'a G,
    directory: &'a R,
    authorisation: &'a A,
}

impl<'a, G, R, A> RunnersService<'a, G, R, A> {
    /// Runners over `adapters`.
    pub const fn new(adapters: RunnersAdapters<'a, G, R, A>) -> Self {
        let RunnersAdapters {
            registry,
            directory,
            authorisation,
        } = adapters;
        Self {
            registry,
            directory,
            authorisation,
        }
    }
}

impl<G, R, A: AuthorisationDecision + Sync> RunnersService<'_, G, R, A> {
    async fn require_admin(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
    ) -> Result<(), RunnersError> {
        let resource = Resource::Organisation(organisation.clone());
        Ok(require(
            self.authorisation,
            caller.principal(),
            Action::AdministerRunners,
            &resource,
        )
        .await?)
    }
}

impl<G, R, A> Runners for RunnersService<'_, G, R, A>
where
    G: RunnerRegistry + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
{
    async fn register(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: Option<&RepositoryName>,
        principal: &Principal,
        labels: BTreeSet<Label>,
    ) -> Result<RegistrationId, RunnersError> {
        self.require_admin(caller, organisation).await?;
        let repository = match repository {
            Some(name) => Some(self.directory.find_repository(organisation, name).await?),
            None => None,
        };
        Ok(self
            .registry
            .register(NewRegistration {
                organisation: organisation.clone(),
                repository,
                principal: principal.clone(),
                labels,
            })
            .await?)
    }

    async fn remove(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        id: RegistrationId,
    ) -> Result<(), RunnersError> {
        self.require_admin(caller, organisation).await?;
        Ok(self.registry.remove(organisation, id).await?)
    }

    async fn list(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
    ) -> Result<Vec<Registration>, RunnersError> {
        self.require_admin(caller, organisation).await?;
        Ok(self.registry.list(organisation).await?)
    }
}
