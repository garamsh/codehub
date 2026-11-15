//! Identity verification: who a presented credential says the caller is (ADR 0006).

use std::fmt;

use crate::domain::credential::Credential;
use crate::domain::principal::Principal;

/// Turns a credential presented with a request into the principal it names.
#[async_trait::async_trait]
pub trait IdentityVerification: Send + Sync {
    /// The principal `credential` identifies.
    ///
    /// # Errors
    ///
    /// [`IdentityError::Rejected`] where the credential does not identify anyone.
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError>;
}

/// Why identity verification refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum IdentityError {
    /// The credential is invalid, expired or unknown.
    Rejected,
    /// The verifier failed; carries a description.
    Backend(String),
}

impl fmt::Display for IdentityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rejected => f.write_str("the credential was rejected"),
            Self::Backend(description) => write!(f, "identity verification failed: {description}"),
        }
    }
}

impl std::error::Error for IdentityError {}
