use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use jsonwebtoken::jwk::{Jwk, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// The signature algorithms a token may use. Everything else, `none` and the HMAC family
/// included, is rejected before a key is looked up.
const ALGORITHMS: [Algorithm; 3] = [Algorithm::RS256, Algorithm::PS256, Algorithm::ES256];

/// Seconds a token is still accepted after its `exp` (and before its `nbf`), for clock skew.
const LEEWAY_SECONDS: u64 = 60;

/// Bound on each request to the issuer.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Identity verification of OIDC access tokens. Discovery and the key set are fetched on first
/// use, cached, and fetched again when a token names a key id the cache lacks.
pub struct OidcIdentityVerification {
    http: reqwest::Client,
    issuer: String,
    validation: Validation,
    keys: Mutex<Keys>,
}

#[derive(Default)]
struct Keys {
    jwks_uri: Option<String>,
    set: JwkSet,
}

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
}

impl OidcIdentityVerification {
    /// A verifier for tokens `issuer` issued for `audience`.
    ///
    /// # Errors
    ///
    /// [`IdentityError::Backend`] where the HTTP client cannot be built.
    pub fn new(issuer: &str, audience: &str) -> Result<Self, IdentityError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(backend)?;
        let mut validation = Validation::new(Algorithm::RS256);
        validation.leeway = LEEWAY_SECONDS;
        validation.validate_nbf = true;
        validation.set_issuer(&[issuer]);
        validation.set_audience(&[audience]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        Ok(Self {
            http,
            issuer: issuer.to_owned(),
            validation,
            keys: Mutex::default(),
        })
    }

    fn cached_key(&self, key_id: &str) -> Option<Jwk> {
        lock(&self.keys).set.find(key_id).cloned()
    }

    /// Discovers the key set's location once, then replaces the cached key set with the issuer's.
    async fn refresh(&self) -> Result<(), IdentityError> {
        let known = lock(&self.keys).jwks_uri.clone();
        let jwks_uri = match known {
            Some(uri) => uri,
            None => {
                let url = format!(
                    "{}/.well-known/openid-configuration",
                    self.issuer.trim_end_matches('/')
                );
                let discovery: Discovery = self.get_json(&url).await?;
                if discovery.issuer != self.issuer {
                    return Err(IdentityError::Backend(format!(
                        "discovery names issuer {}, not {}",
                        discovery.issuer, self.issuer
                    )));
                }
                lock(&self.keys).jwks_uri = Some(discovery.jwks_uri.clone());
                discovery.jwks_uri
            }
        };
        let set: JwkSet = self.get_json(&jwks_uri).await?;
        lock(&self.keys).set = set;
        Ok(())
    }

    async fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T, IdentityError> {
        let response = self.http.get(url).send().await.map_err(backend)?;
        let response = response.error_for_status().map_err(backend)?;
        response.json().await.map_err(backend)
    }
}

impl IdentityVerification for OidcIdentityVerification {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        let token = credential.as_str();
        let header = decode_header(token).map_err(|_| IdentityError::Rejected)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(IdentityError::Rejected);
        }
        let key_id = header.kid.ok_or(IdentityError::Rejected)?;
        let jwk = match self.cached_key(&key_id) {
            Some(jwk) => jwk,
            None => {
                self.refresh().await?;
                self.cached_key(&key_id).ok_or(IdentityError::Rejected)?
            }
        };
        let key = DecodingKey::from_jwk(&jwk).map_err(|_| IdentityError::Rejected)?;
        // `decode` takes one algorithm family per validation, so the token's own algorithm,
        // already checked against the allow-list, is the only one it is given.
        let mut validation = self.validation.clone();
        validation.algorithms = vec![header.alg];
        let data =
            decode::<Claims>(token, &key, &validation).map_err(|_| IdentityError::Rejected)?;
        Principal::new(&data.claims.iss, &data.claims.sub).map_err(|_| IdentityError::Rejected)
    }
}

fn backend(error: reqwest::Error) -> IdentityError {
    IdentityError::Backend(error.to_string())
}

/// Locks `state`; every write replaces a field whole, so a panic while it was held leaves nothing
/// half-applied.
fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
