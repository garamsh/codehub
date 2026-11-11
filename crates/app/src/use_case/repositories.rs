//! The repositories driving port, served by the repository directory, the authorisation decision
//! and the membership it is given.

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, Decision, Resource,
};
use crate::port::driven::membership::{Membership, Role};
use crate::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use crate::port::driving::repositories::{Repositories, RepositoriesError};
use crate::use_case::access::{AccessError, require};

/// Organisations and repositories kept in the directory the composition site passes to it, for
/// the callers the authorisation decision allows.
pub struct RepositoriesService<'a, R, A, M> {
    directory: &'a R,
    authorisation: &'a A,
    membership: &'a M,
}

impl<'a, R, A, M> RepositoriesService<'a, R, A, M> {
    /// Repositories kept in `directory`, callers checked with `authorisation`, and the creator of
    /// an organisation made its admin in `membership`.
    pub const fn new(directory: &'a R, authorisation: &'a A, membership: &'a M) -> Self {
        Self {
            directory,
            authorisation,
            membership,
        }
    }
}

impl<R, A, M> Repositories for RepositoriesService<'_, R, A, M>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    M: Membership + Sync,
{
    async fn create_organisation(
        &self,
        caller: &Caller,
        name: &OrganisationName,
    ) -> Result<(), RepositoriesError> {
        require(
            self.authorisation,
            caller.principal(),
            Action::CreateOrganisation,
            &Resource::Organisation(name.clone()),
        )
        .await?;
        self.directory.create_organisation(name).await?;
        self.membership
            .grant_role(name, caller.principal(), Role::Admin)
            .await?;
        Ok(())
    }

    async fn create_repository(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        require(
            self.authorisation,
            caller.principal(),
            Action::CreateRepository,
            &Resource::Organisation(organisation.clone()),
        )
        .await?;
        Ok(self
            .directory
            .create_repository(organisation, repository)
            .await?)
    }

    async fn find_repository(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        repository: &RepositoryName,
    ) -> Result<RepositoryId, RepositoriesError> {
        let id = self
            .directory
            .find_repository(organisation, repository)
            .await?;
        let resource = Resource::Repository {
            organisation: organisation.clone(),
            repository: id,
        };
        match self
            .authorisation
            .decide(caller.principal(), Action::Read, &resource)
            .await
            .map_err(AccessError::from)?
        {
            Decision::Allow => Ok(id),
            Decision::Deny => Err(DirectoryError::RepositoryNotFound(
                organisation.clone(),
                repository.clone(),
            )
            .into()),
        }
    }
}
