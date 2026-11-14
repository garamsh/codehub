//! The authentication driving port, served by the identity verification it is given.

use crate::domain::credential::Credential;
use crate::domain::principal::Caller;
use crate::port::driven::identity_verification::IdentityVerification;
use crate::port::driving::authentication::{Authentication, AuthenticationError};

/// Authentication by the identity verification the composition site passes to it.
pub struct AuthenticationService<'a, I> {
    identity: &'a I,
}

impl<I> Clone for AuthenticationService<'_, I> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<I> Copy for AuthenticationService<'_, I> {}

impl<'a, I> AuthenticationService<'a, I> {
    /// Authentication that verifies credentials with `identity`.
    pub const fn new(identity: &'a I) -> Self {
        Self { identity }
    }
}

impl<I: IdentityVerification + Sync> Authentication for AuthenticationService<'_, I> {
    async fn authenticate(&self, credential: &Credential) -> Result<Caller, AuthenticationError> {
        Ok(Caller::new(self.identity.verify(credential).await?))
    }
}
