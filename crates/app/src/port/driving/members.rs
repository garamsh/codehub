//! Members: who belongs to an organisation, in which role, and what they may reach.

use std::fmt;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::path::Prefix;
use crate::domain::principal::{Caller, Principal};
use crate::port::driven::membership::{Access, MembershipError, Role};
use crate::port::driven::repository_directory::DirectoryError;
use crate::use_case::access::AccessError;

/// What the outside asks of an organisation's members. Each operation needs an admin of the
/// organisation, and each is refused with [`MembersError::ExternallyAdministered`] where members
/// are administered outside codehub.
pub trait Members {
    /// Makes `principal` a member of `organisation` in `role`, replacing the role it held.
    ///
    /// # Errors
    ///
    /// [`MembersError::Access`] where `caller` is not an admin of `organisation`.
    fn add_member(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        role: Role,
    ) -> impl Future<Output = Result<(), MembersError>> + Send;

    /// Removes `principal` from `organisation` with its grants.
    ///
    /// # Errors
    ///
    /// [`MembersError::Access`] where `caller` is not an admin of `organisation`, and
    /// [`MembersError::Membership`] with [`MembershipError::NotAMember`] where `principal` is
    /// not a member.
    fn remove_member(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
    ) -> impl Future<Output = Result<(), MembersError>> + Send;

    /// Gives the member `principal` `access` on the paths under `prefix` in `repository`.
    ///
    /// # Errors
    ///
    /// [`MembersError::Access`] where `caller` is not an admin of `organisation`,
    /// [`MembersError::Directory`] with [`DirectoryError::RepositoryNotFound`] where the
    /// organisation holds no such repository, and [`MembersError::Membership`] with
    /// [`MembershipError::NotAMember`] where `principal` is not a member.
    fn add_grant(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        repository: &RepositoryName,
        prefix: Prefix,
        access: Access,
    ) -> impl Future<Output = Result<(), MembersError>> + Send;

    /// Takes that grant from the member `principal`; a grant not held is not an error.
    ///
    /// # Errors
    ///
    /// As [`Members::add_grant`].
    fn remove_grant(
        &self,
        caller: &Caller,
        organisation: &OrganisationName,
        principal: &Principal,
        repository: &RepositoryName,
        prefix: Prefix,
        access: Access,
    ) -> impl Future<Output = Result<(), MembersError>> + Send;
}

/// Why the members were not changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MembersError {
    /// The caller may not administer the organisation's members.
    Access(AccessError),
    /// The repository directory refused or failed.
    Directory(DirectoryError),
    /// Membership refused or failed.
    Membership(MembershipError),
    /// Members are administered outside codehub, which keeps none.
    ExternallyAdministered,
}

impl fmt::Display for MembersError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Directory(error) => write!(f, "{error}"),
            Self::Membership(error) => write!(f, "{error}"),
            Self::ExternallyAdministered => {
                write!(f, "members are administered outside codehub")
            }
        }
    }
}

impl std::error::Error for MembersError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Directory(error) => Some(error),
            Self::Membership(error) => Some(error),
            Self::ExternallyAdministered => None,
        }
    }
}

impl From<AccessError> for MembersError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<DirectoryError> for MembersError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}

impl From<MembershipError> for MembersError {
    fn from(error: MembershipError) -> Self {
        Self::Membership(error)
    }
}
