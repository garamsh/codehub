//! Enforcing the authorisation decision: the checks the use cases make before they act.

use std::collections::BTreeSet;

use crate::domain::name::{OrganisationName, RepositoryName};
use crate::domain::path::Path;
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use crate::port::driven::membership::Access;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::access::AccessError;

const fn action(access: Access) -> Action {
    match access {
        Access::Read => Action::Read,
        Access::Write => Action::Write,
    }
}

pub(crate) async fn allowed<A: AuthorisationDecision>(
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

/// The organisation holding `repository` and the name it has there, where `subject` may read it.
pub(crate) async fn readable_repository<R: RepositoryDirectory, A: AuthorisationDecision>(
    directory: &R,
    authorisation: &A,
    subject: &Principal,
    repository: RepositoryId,
) -> Result<(OrganisationName, RepositoryName), AccessError> {
    let not_found = AccessError::RepositoryNotFound(repository);
    let Some((organisation, name)) = directory.names_of(repository).await? else {
        return Err(not_found);
    };
    let resource = Resource::Repository {
        organisation: organisation.clone(),
        name: name.clone(),
        repository,
    };
    if allowed(authorisation, subject, Action::Read, &resource).await? {
        Ok((organisation, name))
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
    name: &RepositoryName,
    repository: RepositoryId,
    access: Access,
    paths: impl IntoIterator<Item = &'p Path>,
) -> Result<(), AccessError> {
    let mut denied = BTreeSet::new();
    for path in paths {
        let resource = Resource::Path {
            organisation: organisation.clone(),
            name: name.clone(),
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
