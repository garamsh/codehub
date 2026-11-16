use std::sync::Mutex;

use app::port::driven::registry_token_issuance::{
    GrantedEntry, IssuedToken, RegistryTokenIssuance, TokenIssuanceError,
};

use crate::lock;

/// What one issued token carries.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IssuedRecord {
    /// The caller's subject.
    pub subject: String,
    /// The registry the token is addressed to.
    pub service: String,
    /// The entries granted.
    pub entries: Vec<GrantedEntry>,
    /// When the token was issued, in seconds since 1970-01-01T00:00:00Z.
    pub issued_at: i64,
    /// When it expires, in the same unit.
    pub expires_at: i64,
}

/// Token issuance that signs nothing: its token is `memory-token-<n>`, and the harness reads
/// what each token carries back with [`MemoryRegistryTokenIssuance::carried_by`].
#[derive(Default)]
pub struct MemoryRegistryTokenIssuance {
    issued: Mutex<Vec<IssuedRecord>>,
}

impl MemoryRegistryTokenIssuance {
    /// What the token `token` carries; `None` where this adapter issued no such token.
    #[must_use]
    pub fn carried_by(&self, token: &str) -> Option<IssuedRecord> {
        let number: usize = token.strip_prefix("memory-token-")?.parse().ok()?;
        lock(&self.issued).get(number).cloned()
    }
}

#[async_trait::async_trait]
impl RegistryTokenIssuance for MemoryRegistryTokenIssuance {
    async fn issue(
        &self,
        subject: &str,
        service: &str,
        entries: &[GrantedEntry],
        issued_at: i64,
        expires_at: i64,
    ) -> Result<IssuedToken, TokenIssuanceError> {
        let mut issued = lock(&self.issued);
        issued.push(IssuedRecord {
            subject: subject.to_owned(),
            service: service.to_owned(),
            entries: entries.to_vec(),
            issued_at,
            expires_at,
        });
        Ok(IssuedToken {
            token: format!("memory-token-{}", issued.len() - 1),
            expires_in: u64::try_from(expires_at - issued_at).unwrap_or(0),
        })
    }
}
