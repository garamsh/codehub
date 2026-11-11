//! The members driving port, served by the directory, the authorisation decision and the
//! membership it is given.

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::path::Prefix;
use crate::domain::principal::{Caller, Principal};
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::membership::{Access, Grant, Membership, Role};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::members::{Members, MembersError};
use crate::use_case::access::require;
use crate::use_case::administration::Administration;

/// Members kept in the membership the composition site passes to it, administered by the callers
/// the authorisation decision allows.
pub struct MembersService<'a, R, A, M> {
    directory: &'a R,
    authorisation: &'a A,
    membership: &'a M,
    administration: Administration,
}

impl<'a, R, A, M> MembersService<'a, R, A, M> {
    /// Members kept in `membership`, with repositories looked up in `directory` and callers
    /// checked with `authorisation`, where `administration` says whether members are kept here
    /// at all.
    pub const fn new(
        directory: &'a R,
        authorisation: &'a A,
        membership: &'a M,
        administration: Administration,
    ) -> Self {
        Self {
            directory,
            authorisation,
            membership,
            administration,
        }
    }
}

impl<R, A, M> MembersService<'_, R, A, M>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
{
    /// Requires that members are kept here and that `caller` administers them in `organisation`.
    async fn require_admin(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
    ) -> Result<(), MembersError> {
        if self.administration == Administration::External {
            return Err(MembersError::ExternallyAdministered);
        }
        let resource = Resource::Organisation(organisation.clone());
        Ok(require(
            self.authorisation,
            caller.principal(),
            Action::AdministerMembers,
            &resource,
        )
        .await?)
    }

    async fn grant(
        &self,
        organisation: &OrganisationName,
        repository: &RepositoryName,
        prefix: Prefix,
        access: Access,
    ) -> Result<Grant, MembersError> {
        Ok(Grant {
            repository: self
                .directory
                .find_repository(organisation, repository)
                .await?,
            prefix,
            access,
        })
    }
}

impl<R, A, M> Members for MembersService<'_, R, A, M>
where
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    M: Membership + Sync,
{
    async fn add_member(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        role: Role,
    ) -> Result<(), MembersError> {
        self.require_admin(caller, organisation).await?;
        Ok(self
            .membership
            .grant_role(organisation, principal, role)
            .await?)
    }

    async fn remove_member(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
    ) -> Result<(), MembersError> {
        self.require_admin(caller, organisation).await?;
        Ok(self.membership.revoke_role(organisation, principal).await?)
    }

    async fn add_grant(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        repository: &RepositoryName,
        prefix: Prefix,
        access: Access,
    ) -> Result<(), MembersError> {
        self.require_admin(caller, organisation).await?;
        let grant = self.grant(organisation, repository, prefix, access).await?;
        Ok(self
            .membership
            .add_grant(organisation, principal, grant)
            .await?)
    }

    async fn remove_grant(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        repository: &RepositoryName,
        prefix: Prefix,
        access: Access,
    ) -> Result<(), MembersError> {
        self.require_admin(caller, organisation).await?;
        let grant = self.grant(organisation, repository, prefix, access).await?;
        Ok(self
            .membership
            .remove_grant(organisation, principal, &grant)
            .await?)
    }
}
