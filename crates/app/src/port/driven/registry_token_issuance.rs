//! Registry token issuance: the signed token an image registry accepts as the caller's grant
//! (ADR 0053).

use std::fmt;

use crate::domain::image::ImageAction;

/// What the token allows on one name of one type, in the application's terms.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct GrantedEntry {
    /// What the name names, as the client wrote it; `repository` for an image's repository.
    pub kind: String,
    /// The name, as the client wrote it.
    pub name: String,
    /// The actions allowed; none where the client was granted nothing on this name.
    pub actions: Vec<ImageAction>,
}

/// A signed token and how long it lasts.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IssuedToken {
    /// The token, opaque to the application.
    pub token: String,
    /// Seconds from the issue time to the token's expiry.
    pub expires_in: u64,
}

/// Signs the grants of a caller into a token the registry accepts.
#[async_trait::async_trait]
pub trait RegistryTokenIssuance: Send + Sync {
    /// A token for `subject`, addressed to the registry `service`, carrying `entries`, valid
    /// from `issued_at` until `expires_at`; both are seconds since 1970-01-01T00:00:00Z.
    ///
    /// # Errors
    ///
    /// [`TokenIssuanceError::Backend`] where the token could not be signed.
    async fn issue(
        &self,
        subject: &str,
        service: &str,
        entries: &[GrantedEntry],
        issued_at: i64,
        expires_at: i64,
    ) -> Result<IssuedToken, TokenIssuanceError>;
}

/// Why no token was issued.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TokenIssuanceError {
    /// The signer failed; carries a description.
    Backend(String),
}

impl fmt::Display for TokenIssuanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(description) => write!(f, "token issuance failed: {description}"),
        }
    }
}

impl std::error::Error for TokenIssuanceError {}
