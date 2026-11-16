//! The registry token: what an image registry's client asks of the token service (ADR 0053).

use std::fmt;

use crate::domain::principal::Caller;
use crate::port::driven::authorisation_decision::AuthorisationError;
use crate::port::driven::registry_token_issuance::TokenIssuanceError;
use crate::port::driven::repository_directory::DirectoryError;

/// The most scopes one request may carry.
pub const MAX_SCOPES: usize = 16;

/// What the client asks for.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RegistryTokenRequest {
    /// The registry the token is for.
    pub service: String,
    /// The scopes asked for, each as the client wrote it.
    pub scopes: Vec<String>,
}

/// The token the client is given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RegistryTokenGrant {
    /// The token.
    pub token: String,
    /// Seconds the token lasts.
    pub expires_in: u64,
    /// When it was issued, as RFC 3339 text in UTC.
    pub issued_at: String,
}

/// What the outside asks of the registry's token service.
#[async_trait::async_trait]
pub trait RegistryToken: Send + Sync {
    /// A token for `caller` carrying what each scope of `request` is allowed to it. A scope that
    /// is malformed, names an unknown repository or asks for an action other than `pull` and
    /// `push` is allowed nothing, as a denied one is.
    ///
    /// # Errors
    ///
    /// [`RegistryTokenError::WrongService`] where the service is not the one served,
    /// [`RegistryTokenError::TooManyScopes`] where there are more than [`MAX_SCOPES`], and the
    /// other variants where a port failed.
    async fn issue(
        &self,
        caller: &Caller,
        request: &RegistryTokenRequest,
    ) -> Result<RegistryTokenGrant, RegistryTokenError>;
}

/// Why no token was given.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RegistryTokenError {
    /// The service is not the one the token service serves.
    WrongService,
    /// The request carries more than [`MAX_SCOPES`] scopes.
    TooManyScopes,
    /// The directory failed.
    Directory(DirectoryError),
    /// The authorisation decision failed.
    Authorisation(AuthorisationError),
    /// The token could not be signed.
    Issuance(TokenIssuanceError),
}

impl fmt::Display for RegistryTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongService => f.write_str("the service is not the one this registry serves"),
            Self::TooManyScopes => write!(f, "a request carries at most {MAX_SCOPES} scopes"),
            Self::Directory(error) => write!(f, "{error}"),
            Self::Authorisation(error) => write!(f, "{error}"),
            Self::Issuance(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RegistryTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::WrongService | Self::TooManyScopes => None,
            Self::Directory(error) => Some(error),
            Self::Authorisation(error) => Some(error),
            Self::Issuance(error) => Some(error),
        }
    }
}
