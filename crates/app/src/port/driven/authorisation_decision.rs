//! The authorisation decision: whether a principal may take an action on a resource. The
//! application enforces the answer and evaluates no policy (ADR 0006, ADR 0021).

use std::fmt;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::path::Path;
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;

/// What a principal asks to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Create an organisation; asked of an [`Resource::Organisation`].
    CreateOrganisation,
    /// Create a repository; asked of an [`Resource::Organisation`].
    CreateRepository,
    /// Add or remove members and their grants; asked of an [`Resource::Organisation`].
    AdministerMembers,
    /// Register, list and remove runners, and cancel jobs; asked of an [`Resource::Organisation`].
    AdministerRunners,
    /// Read; asked of a [`Resource::Repository`] or a [`Resource::Path`].
    Read,
    /// Write; asked of a [`Resource::Repository`] or a [`Resource::Path`].
    Write,
}

/// What the action is on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Resource {
    /// An organisation.
    Organisation(OrganisationName),
    /// A repository, as a whole.
    Repository {
        /// The organisation holding it.
        organisation: OrganisationName,
        /// The repository's name in the organisation, by which an outside decision point knows it.
        name: RepositoryName,
        /// The repository.
        repository: RepositoryId,
    },
    /// A path in a repository.
    Path {
        /// The organisation holding the repository.
        organisation: OrganisationName,
        /// The repository's name in the organisation, by which an outside decision point knows it.
        name: RepositoryName,
        /// The repository.
        repository: RepositoryId,
        /// The path in it.
        path: Path,
    },
}

/// The answer to a decision request.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// The principal may.
    Allow,
    /// The principal may not.
    Deny,
}

/// Decides whether an action on a resource is allowed.
pub trait AuthorisationDecision {
    /// Whether `subject` may take `action` on `resource`. A pairing the action does not apply to
    /// is denied.
    ///
    /// # Errors
    ///
    /// [`AuthorisationError::Backend`] where the decision could not be made; callers treat it as
    /// a refusal.
    fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> impl Future<Output = Result<Decision, AuthorisationError>> + Send;
}

/// Why no decision was made.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuthorisationError {
    /// The decision point failed; carries a description.
    Backend(String),
}

impl fmt::Display for AuthorisationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(description) => write!(f, "authorisation failed: {description}"),
        }
    }
}

impl std::error::Error for AuthorisationError {}
