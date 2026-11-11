use std::collections::BTreeMap;
use std::sync::Mutex;

use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};

use crate::lock;

/// Identity verification over a fixed set of credentials, each registered with the principal it
/// names.
#[derive(Default)]
pub struct MemoryIdentityVerification {
    principals: Mutex<BTreeMap<String, Principal>>,
}

impl MemoryIdentityVerification {
    /// Makes `credential` identify `principal`.
    pub fn register(&self, credential: &Credential, principal: Principal) {
        lock(&self.principals).insert(credential.as_str().to_owned(), principal);
    }
}

impl IdentityVerification for MemoryIdentityVerification {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        lock(&self.principals)
            .get(credential.as_str())
            .cloned()
            .ok_or(IdentityError::Rejected)
    }
}
