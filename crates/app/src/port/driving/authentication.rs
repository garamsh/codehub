//! Authentication: a presented credential turned into the caller every other operation takes.

use std::fmt;

use crate::domain::credential::Credential;
use crate::domain::principal::Caller;
use crate::port::driven::identity_verification::IdentityError;

/// What the outside asks to establish who is calling.
#[async_trait::async_trait]
pub trait Authentication: Send + Sync {
    /// The caller `credential` identifies.
    ///
    /// # Errors
    ///
    /// [`AuthenticationError::Identity`] with [`IdentityError::Rejected`] where the credential
    /// does not identify anyone.
    async fn authenticate(&self, credential: &Credential) -> Result<Caller, AuthenticationError>;
}

/// Why no caller was established.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AuthenticationError {
    /// Identity verification refused or failed.
    Identity(IdentityError),
}

impl fmt::Display for AuthenticationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Identity(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AuthenticationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Identity(error) => Some(error),
        }
    }
}

impl From<IdentityError> for AuthenticationError {
    fn from(error: IdentityError) -> Self {
        Self::Identity(error)
    }
}
