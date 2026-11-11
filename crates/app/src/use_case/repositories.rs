//! The repositories driving port, served by the repository directory, the authorisation decision,
//! the membership and the directory changes it is given.

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, Decision, Resource,
};
use crate::port::driven::directory_changes::{DirectoryChange, DirectoryChanges};
use crate::port::driven::membership::{Membership, Role};
use crate::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use crate::port::driving::repositories::{Repositories, RepositoriesError};
use crate::use_case::access::{AccessError, allowed, require};
use crate::use_case::administration::Administration;

/// Organisations and repositories kept in the directory the composition site passes to it, for
/// the callers the authorisation decision allows, each creation reported to the directory
/// changes.
pub struct RepositoriesService<'a, R, A, M, C> {
    directory: &'a R,
    authorisation: &'a A,
    membership: &'a M,
    changes: &'a C,
    administration: Administration,
}

impl<'a, R, A, M, C> RepositoriesService<'a, R, A, M, C> {
    /// Repositories kept in `directory`, callers checked with `authorisation`, and each creation
    /// reported to `changes`. Where `administration` is built-in, the creator of an organisation
    /// is made its admin in `membership`; where it is external, no grant is written and an
    /// organisation missing from the directory is created on first use.
    pub const fn new(
        directory: &'a R,
        authorisation: &'a A,
        membership: &'a M,
        changes: &'a C,
        administration: Administration,
    ) -> Self {
        Self {
            directory,
            authorisation,
            membership,
            changes,
            administration,
        }
    }
}

impl<R, A, M, C> RepositoriesService<'_, R, A, M, C>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    C: DirectoryChanges + Sync,
{
    /// Creates `organisation`, which the directory did not hold (`missing`), where `caller` may
    /// create it; otherwise returns `missing`. An organisation a concurrent creator made first
    /// counts as created and is not reported.
    async fn create_missing_organisation(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        missing: DirectoryError,
    ) -> Result<(), RepositoriesError> {
        let resource = Resource::Organisation(organisation.clone());
        let may_create = allowed(
            self.authorisation,
            caller.principal(),
            Action::CreateOrganisation,
            &resource,
        )
        .await
        .map_err(AccessError::from)?;
        if !may_create {
            return Err(missing.into());
        }
        match self.directory.create_organisation(organisation).await {
            Ok(()) => {
                self.changes
                    .report(&DirectoryChange::OrganisationCreated(organisation.clone()))
                    .await;
                Ok(())
            }
            Err(DirectoryError::OrganisationExists(_)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl<R, A, M, C> Repositories for RepositoriesService<'_, R, A, M, C>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    M: Membership + Sync,
    C: DirectoryChanges + Sync,
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
        self.changes
            .report(&DirectoryChange::OrganisationCreated(name.clone()))
            .await;
        if self.administration == Administration::BuiltIn {
            self.membership
                .grant_role(name, caller.principal(), Role::Admin)
                .await?;
        }
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
        let id = match self
            .directory
            .create_repository(organisation, repository)
            .await
        {
            Err(missing @ DirectoryError::OrganisationNotFound(_))
                if self.administration == Administration::External =>
            {
                self.create_missing_organisation(caller, organisation, missing)
                    .await?;
                self.directory
                    .create_repository(organisation, repository)
                    .await?
            }
            created => created?,
        };
        self.changes
            .report(&DirectoryChange::RepositoryCreated {
                organisation: organisation.clone(),
                repository: repository.clone(),
            })
            .await;
        Ok(id)
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
            name: repository.clone(),
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
