//! The failure shared by the driving ports: the caller may not do what it asked.

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::path::Path;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationError};
use crate::port::driven::membership::Access;
use crate::port::driven::repository_directory::DirectoryError;

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
