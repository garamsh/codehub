//! The members driving port, served by the directory, the authorisation decision and the
//! membership it is given.

use std::sync::Arc;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::path::Prefix;
use crate::domain::principal::{Caller, Principal};
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::membership::{Access, Grant, Membership, Role};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::members::{Members, MembersError};
use crate::use_case::access::require;
use crate::use_case::administration::Administration;

/// The driven ports the members are built over.
pub struct MembersAdapters {
    /// Looks repositories up.
    pub directory: Arc<dyn RepositoryDirectory>,
    /// Says what callers may do.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Keeps members and their grants.
    pub membership: Arc<dyn Membership>,
}

/// Members kept in the membership the composition site passes to it, administered by the callers
/// the authorisation decision allows.
pub struct MembersService {
    directory: Arc<dyn RepositoryDirectory>,
    authorisation: Arc<dyn AuthorisationDecision>,
    membership: Arc<dyn Membership>,
    administration: Administration,
}

impl MembersService {
    /// Members over `adapters`, where `administration` says whether members are kept here at
    /// all.
    pub fn new(adapters: MembersAdapters, administration: Administration) -> Self {
        let MembersAdapters {
            directory,
            authorisation,
            membership,
        } = adapters;
        Self {
            directory,
            authorisation,
            membership,
            administration,
        }
    }
}

impl MembersService {
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
            &*self.authorisation,
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
        let whole_only = matches!(access, Access::Run | Access::ImagePull | Access::ImagePush);
        if whole_only && prefix != Prefix::whole() {
            return Err(MembersError::WholeRepositoryGrantOnPrefix);
        }
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

#[async_trait::async_trait]
impl Members for MembersService {
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
