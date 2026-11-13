//! The repositories driving port, served by the repository directory, the authorisation decision,
//! the membership and the directory changes it is given, and the delivery of a repository's outbox
//! entries once its creation has committed (ADR 0044).

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, Decision, Resource,
};
use crate::port::driven::directory_changes::{DirectoryChanges, OrganisationChange};
use crate::port::driven::membership::{Membership, Role};
use crate::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};
use crate::port::driving::access::AccessError;
use crate::port::driving::repositories::{Repositories, RepositoriesError};
use crate::use_case::access::{allowed, require};
use crate::use_case::administration::Administration;
use crate::use_case::relay::Delivery;

/// The driven ports the repositories are built over.
pub struct RepositoriesAdapters<'a, R, A, M, C, D> {
    /// Keeps organisations and repositories.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
    /// Keeps members and their grants.
    pub membership: &'a M,
    /// Receives each organisation creation.
    pub changes: &'a C,
    /// Delivers the entries of a repository's creation once it has committed.
    pub delivery: &'a D,
}

/// Organisations and repositories kept in the directory the composition site passes to it, for
/// the callers the authorisation decision allows, each organisation creation reported to the
/// directory changes and each repository creation followed by the delivery of its entries.
pub struct RepositoriesService<'a, R, A, M, C, D> {
    directory: &'a R,
    authorisation: &'a A,
    membership: &'a M,
    changes: &'a C,
    delivery: &'a D,
    administration: Administration,
}

impl<'a, R, A, M, C, D> RepositoriesService<'a, R, A, M, C, D> {
    /// Repositories over `adapters`. Where `administration` is built-in, the creator of an
    /// organisation is made its admin in the membership; where it is external, no grant is
    /// written and an organisation missing from the directory is created on first use.
    pub const fn new(
        adapters: RepositoriesAdapters<'a, R, A, M, C, D>,
        administration: Administration,
    ) -> Self {
        let RepositoriesAdapters {
            directory,
            authorisation,
            membership,
            changes,
            delivery,
        } = adapters;
        Self {
            directory,
            authorisation,
            membership,
            changes,
            delivery,
            administration,
        }
    }
}

impl<R, A, M, C, D> RepositoriesService<'_, R, A, M, C, D>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    C: DirectoryChanges + Sync,
    D: Delivery + Sync,
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
                    .organisation_changed(&OrganisationChange::Created(organisation.clone()))
                    .await;
                Ok(())
            }
            Err(DirectoryError::OrganisationExists(_)) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl<R, A, M, C, D> Repositories for RepositoriesService<'_, R, A, M, C, D>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    M: Membership + Sync,
    C: DirectoryChanges + Sync,
    D: Delivery + Sync,
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
            .organisation_changed(&OrganisationChange::Created(name.clone()))
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
        self.delivery.deliver(id).await;
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
