use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use futures_util::future::{BoxFuture, FutureExt, Shared};
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
/// use, cached, and fetched again when a token names a key id the cache lacks. One fetch runs at a
/// time, and a token that finds no key meanwhile waits for it.
pub struct OidcIdentityVerification {
    http: reqwest::Client,
    issuer: String,
    validation: Validation,
    min_refetch_interval: Duration,
    keys: Arc<Mutex<Keys>>,
}

#[derive(Default)]
struct Keys {
    /// When the last fetch finished, whether it succeeded or not.
    fetched_at: Option<Instant>,
    jwks_uri: Option<String>,
    set: JwkSet,
    fetching: Option<Fetch>,
}

/// A key-set fetch in flight, which every verification waiting for it polls.
type Fetch = Shared<BoxFuture<'static, Result<(), IdentityError>>>;

/// What a verification found for a token's key id.
enum Lookup {
    Cached(Jwk),
    Pending(Fetch),
}

struct FetchedKeys {
    jwks_uri: String,
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
    /// A verifier for tokens `issuer` issued for `audience`. The key set is fetched again for an
    /// unknown key id at most once per `min_refetch_interval`.
    ///
    /// # Errors
    ///
    /// [`IdentityError::Backend`] where the HTTP client cannot be built.
    pub fn new(
        issuer: &str,
        audience: &str,
        min_refetch_interval: Duration,
    ) -> Result<Self, IdentityError> {
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
            min_refetch_interval,
            keys: Arc::default(),
        })
    }

    fn cached_key(&self, key_id: &str) -> Option<Jwk> {
        lock(&self.keys).set.find(key_id).cloned()
    }

    /// The cached key for `key_id`, else the fetch to wait for, started here when none is in
    /// flight. Rejects an unknown key id when a fetch finished too recently to fetch again.
    fn lookup(&self, key_id: &str) -> Result<Lookup, IdentityError> {
        let mut state = lock(&self.keys);
        if let Some(jwk) = state.set.find(key_id) {
            return Ok(Lookup::Cached(jwk.clone()));
        }
        if let Some(fetch) = &state.fetching {
            return Ok(Lookup::Pending(fetch.clone()));
        }
        if state
            .fetched_at
            .is_some_and(|at| at.elapsed() < self.min_refetch_interval)
        {
            return Err(IdentityError::Rejected);
        }
        let fetch = self.fetch(state.jwks_uri.clone());
        state.fetching = Some(fetch.clone());
        Ok(Lookup::Pending(fetch))
    }

    /// A fetch that, when it finishes, stores its outcome and its finish time and ends itself.
    /// It holds the state weakly, so a fetch no verification polls does not keep it alive.
    fn fetch(&self, known_uri: Option<String>) -> Fetch {
        let (http, issuer) = (self.http.clone(), self.issuer.clone());
        let keys = Arc::downgrade(&self.keys);
        async move {
            let fetched = fetch_keys(&http, &issuer, known_uri).await;
            let Some(keys) = keys.upgrade() else {
                return fetched.map(|_| ());
            };
            let mut state = lock(&keys);
            state.fetched_at = Some(Instant::now());
            state.fetching = None;
            fetched.map(|fetched| {
                state.jwks_uri = Some(fetched.jwks_uri);
                state.set = fetched.set;
            })
        }
        .boxed()
        .shared()
    }
}

/// Discovers the key set's location unless `known_uri` has it, then fetches the key set.
async fn fetch_keys(
    http: &reqwest::Client,
    issuer: &str,
    known_uri: Option<String>,
) -> Result<FetchedKeys, IdentityError> {
    let jwks_uri = match known_uri {
        Some(uri) => uri,
        None => {
            let url = format!(
                "{}/.well-known/openid-configuration",
                issuer.trim_end_matches('/')
            );
            let discovery: Discovery = get_json(http, &url).await?;
            if discovery.issuer != issuer {
                return Err(IdentityError::Backend(format!(
                    "discovery names issuer {}, not {issuer}",
                    discovery.issuer
                )));
            }
            discovery.jwks_uri
        }
    };
    let set = get_json(http, &jwks_uri).await?;
    Ok(FetchedKeys { jwks_uri, set })
}

async fn get_json<T: DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
) -> Result<T, IdentityError> {
    let response = http.get(url).send().await.map_err(backend)?;
    let response = response.error_for_status().map_err(backend)?;
    response.json().await.map_err(backend)
}

impl IdentityVerification for OidcIdentityVerification {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        let token = credential.as_str();
        let header = decode_header(token).map_err(|_| IdentityError::Rejected)?;
        if !ALGORITHMS.contains(&header.alg) {
            return Err(IdentityError::Rejected);
        }
        let key_id = header.kid.ok_or(IdentityError::Rejected)?;
        let jwk = match self.lookup(&key_id)? {
            Lookup::Cached(jwk) => jwk,
            Lookup::Pending(fetch) => {
                fetch.await?;
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
