//! Membership: who belongs to an organisation in which role, and the grants its members hold
//! (ADR 0021).

use std::fmt;

use crate::domain::name::OrganisationName;
use crate::domain::path::Prefix;
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;

/// A principal's role in an organisation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    /// May take every action on the organisation and its repositories.
    Admin,
    /// May take the actions its grants allow.
    Member,
}

/// What a grant allows; `Write` includes `Read`; `Run`, `ImagePull` and `ImagePush` include nothing else.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Access {
    /// Read.
    Read,
    /// Read and write.
    Write,
    /// Run a command on a runner against the caller's workspace (ADR 0051); held for the whole
    /// repository only.
    Run,
    /// Pull the repository's images from the registry (ADR 0053); held for the whole repository
    /// only.
    ImagePull,
    /// Push the repository's images to the registry (ADR 0053); held for the whole repository
    /// only, and not including `ImagePull`.
    ImagePush,
}

/// Permission for a member over the paths of one repository under a prefix.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Grant {
    /// The repository the grant is in.
    pub repository: RepositoryId,
    /// The paths it covers.
    pub prefix: Prefix,
    /// What it allows on them.
    pub access: Access,
}

/// Holds the roles and grants of each organisation's members.
#[async_trait::async_trait]
pub trait Membership: Send + Sync {
    /// Makes `principal` a member of `organisation` in `role`, replacing the role it held.
    ///
    /// # Errors
    ///
    /// The store's [`MembershipError`].
    async fn grant_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        role: Role,
    ) -> Result<(), MembershipError>;

    /// Removes `principal` from `organisation`, with every grant it held there.
    ///
    /// # Errors
    ///
    /// [`MembershipError::NotAMember`] where `principal` holds no role in `organisation`.
    async fn revoke_role(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
    ) -> Result<(), MembershipError>;

    /// Gives the member `principal` of `organisation` the grant `grant`.
    ///
    /// # Errors
    ///
    /// [`MembershipError::NotAMember`] where `principal` holds no role in `organisation`.
    async fn add_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: Grant,
    ) -> Result<(), MembershipError>;

    /// Takes the grant `grant` from the member `principal` of `organisation`; a grant not held
    /// is not an error.
    ///
    /// # Errors
    ///
    /// [`MembershipError::NotAMember`] where `principal` holds no role in `organisation`.
    async fn remove_grant(
        &self,
        organisation: &OrganisationName,
        principal: &Principal,
        grant: &Grant,
    ) -> Result<(), MembershipError>;
}

/// Why membership refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MembershipError {
    /// The principal holds no role in the organisation.
    NotAMember(Principal),
    /// The store failed; carries a description.
    Backend(String),
}

impl fmt::Display for MembershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAMember(principal) => write!(f, "{principal} is not a member"),
            Self::Backend(description) => write!(f, "membership failed: {description}"),
        }
    }
}

impl std::error::Error for MembershipError {}
