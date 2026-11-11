//! Enforcing the authorisation decision: the checks the use cases make before they act.

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::name::OrganisationName;
use crate::domain::path::Path;
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use crate::port::driven::membership::Access;
use crate::port::driven::repository_directory::{DirectoryError, RepositoryDirectory};

/// Why a caller may not do what it asked.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AccessError {
    /// The repository does not exist, or the caller holds no read on it; the two are not told
    /// apart.
    RepositoryNotFound(RepositoryId),
    /// The caller may not take the action.
    Denied(Action),
    /// The caller lacks the access on each of these paths.
    PathsDenied {
        /// What the caller asked for on them.
        access: Access,
        /// Every path denied.
        paths: BTreeSet<Path>,
    },
    /// The repository directory failed.
    Directory(DirectoryError),
    /// The authorisation decision failed, which refuses the call.
    Authorisation(AuthorisationError),
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RepositoryNotFound(repository) => write!(f, "no repository {repository}"),
            Self::Denied(action) => write!(f, "not allowed: {action:?}"),
            Self::PathsDenied { access, paths } => {
                write!(f, "{access:?} denied on")?;
                for path in paths {
                    write!(f, " {path}")?;
                }
                Ok(())
            }
            Self::Directory(error) => write!(f, "{error}"),
            Self::Authorisation(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AccessError {}

impl From<DirectoryError> for AccessError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}

impl From<AuthorisationError> for AccessError {
    fn from(error: AuthorisationError) -> Self {
        Self::Authorisation(error)
    }
}

const fn action(access: Access) -> Action {
    match access {
        Access::Read => Action::Read,
        Access::Write => Action::Write,
    }
}

async fn allowed<A: AuthorisationDecision>(
    authorisation: &A,
    subject: &Principal,
    action: Action,
    resource: &Resource,
) -> Result<bool, AuthorisationError> {
    Ok(authorisation.decide(subject, action, resource).await? == Decision::Allow)
}

/// Requires that `subject` may take `action` on `resource`.
pub(crate) async fn require<A: AuthorisationDecision>(
    authorisation: &A,
    subject: &Principal,
    action: Action,
    resource: &Resource,
) -> Result<(), AccessError> {
    if allowed(authorisation, subject, action, resource).await? {
        Ok(())
    } else {
        Err(AccessError::Denied(action))
    }
}

/// The organisation holding `repository`, where `subject` may read it.
pub(crate) async fn readable_repository<R: RepositoryDirectory, A: AuthorisationDecision>(
    directory: &R,
    authorisation: &A,
    subject: &Principal,
    repository: RepositoryId,
) -> Result<OrganisationName, AccessError> {
    let not_found = AccessError::RepositoryNotFound(repository);
    let Some(organisation) = directory.organisation_of(repository).await? else {
        return Err(not_found);
    };
    let resource = Resource::Repository {
        organisation: organisation.clone(),
        repository,
    };
    if allowed(authorisation, subject, Action::Read, &resource).await? {
        Ok(organisation)
    } else {
        Err(not_found)
    }
}

/// Requires that `subject` holds `access` on every one of `paths` in `repository`, and names all
/// that it does not.
pub(crate) async fn require_paths<'p, A: AuthorisationDecision>(
    authorisation: &A,
    subject: &Principal,
    organisation: &OrganisationName,
    repository: RepositoryId,
    access: Access,
    paths: impl IntoIterator<Item = &'p Path>,
) -> Result<(), AccessError> {
    let mut denied = BTreeSet::new();
    for path in paths {
        let resource = Resource::Path {
            organisation: organisation.clone(),
            repository,
            path: path.clone(),
        };
        if !allowed(authorisation, subject, action(access), &resource).await? {
            denied.insert(path.clone());
        }
    }
    if denied.is_empty() {
        Ok(())
    } else {
        Err(AccessError::PathsDenied {
            access,
            paths: denied,
        })
    }
}
