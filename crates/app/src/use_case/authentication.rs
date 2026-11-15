//! The authentication driving port, served by the identity verification it is given.

use std::sync::Arc;

use crate::domain::credential::Credential;
use crate::domain::principal::Caller;
use crate::port::driven::identity_verification::IdentityVerification;
use crate::port::driving::authentication::{Authentication, AuthenticationError};

/// Authentication by the identity verification the composition site passes to it.
#[derive(Clone)]
pub struct AuthenticationService {
    identity: Arc<dyn IdentityVerification>,
}

impl AuthenticationService {
    /// Authentication that verifies credentials with `identity`.
    pub fn new(identity: Arc<dyn IdentityVerification>) -> Self {
        Self { identity }
    }
}

#[async_trait::async_trait]
impl Authentication for AuthenticationService {
    async fn authenticate(&self, credential: &Credential) -> Result<Caller, AuthenticationError> {
        Ok(Caller::new(self.identity.verify(credential).await?))
    }
}
