//! Authentication: a presented credential turned into the caller every other operation takes.

use std::fmt;

use crate::domain::credential::Credential;
use crate::domain::principal::Caller;
use crate::port::driven::identity_verification::IdentityError;

/// What the outside asks to establish who is calling.
pub trait Authentication {
    /// The caller `credential` identifies.
    ///
    /// # Errors
    ///
    /// [`AuthenticationError::Identity`] with [`IdentityError::Rejected`] where the credential
    /// does not identify anyone.
    fn authenticate(
        &self,
        credential: &Credential,
    ) -> impl Future<Output = Result<Caller, AuthenticationError>> + Send;
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
