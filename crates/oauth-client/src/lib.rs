//! An OAuth 2.0 client that obtains bearer tokens by the client credentials grant (RFC 6749 §4.4)
//! from one token endpoint (ADR 0042, ADR 0044).
//!
//! The client authenticates by a client secret, with HTTP Basic, or by a signed assertion
//! (`private_key_jwt`, RFC 7523 as updated by RFC 10070). A token is asked for an organisation:
//! with an assertion the client id names the organisation and each organisation holds its own
//! token, and with a client secret one token serves every organisation. Tokens are held in memory
//! until they are within a few seconds of their `expires_in`; a refusal of the client as
//! `unauthorized_client` is a distinct outcome, and is never held.
//!
//! The client id is sent as the `client_id` parameter. The assertion's `iss` and `sub` are the
//! client id, its `aud` is the issuer identifier as one string, its lifetime is 30 seconds, it
//! carries a fresh `jti`, and its header has `typ` `client-authentication+jwt` and, where
//! configured, `kid`. The algorithm follows the key: ES256 for a P-256 key and RS256 for an RSA
//! key, both PEM PKCS#8.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

/// Bound on each request to the token endpoint.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A token is replaced when it is this close to its expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(5);

/// How long an assertion is valid.
const ASSERTION_LIFETIME: Duration = Duration::from_secs(30);

/// The placeholder of a client id template that the organisation replaces.
pub const ORGANISATION_PLACEHOLDER: &str = "{organisation}";

/// The `typ` header of a client authentication assertion (RFC 10070).
const ASSERTION_TYPE: &str = "client-authentication+jwt";

/// The `client_assertion_type` of an assertion signed by the client (RFC 7523 §2.2).
const CLIENT_ASSERTION_TYPE: &str = "urn:ietf:params:oauth:client-assertion-type:jwt-bearer";

/// How the client proves who it is to the token endpoint.
pub enum Authentication {
    /// HTTP Basic, with a client id and secret that serve every organisation.
    ClientSecret {
        /// The client id.
        client_id: String,
        /// The client secret.
        client_secret: String,
    },
    /// A signed assertion for each token request, one client per organisation.
    PrivateKeyJwt {
        /// The client id with [`ORGANISATION_PLACEHOLDER`] where the organisation goes.
        client_id_template: String,
        /// The private key, PEM PKCS#8, RSA or P-256.
        key_pem: Vec<u8>,
        /// The `kid` header of the assertion, none where absent.
        key_id: Option<String>,
        /// The issuer identifier of the token endpoint's server, the assertion's `aud`.
        issuer: String,
    },
}

/// Why a client cannot be built.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidClient(String);

impl fmt::Display for InvalidClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for InvalidClient {}

/// Why no token was obtained.
#[derive(Debug, PartialEq, Eq)]
pub enum TokenError {
    /// The token endpoint refused the client as `unauthorized_client` (RFC 6749 §5.2): it has no
    /// authority to ask for a token, which for an organisation means it is not set up for it.
    UnauthorizedClient,
    /// The token could not be obtained for any other reason; carries a description.
    Failed(String),
}

impl fmt::Display for TokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnauthorizedClient => f.write_str("the token endpoint refused the client"),
            Self::Failed(description) => f.write_str(description),
        }
    }
}

impl std::error::Error for TokenError {}

/// A bearer token. Its value shows only in [`Token::as_str`], never in `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl Token {
    /// The token as it goes in `Authorization: Bearer <token>`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(..)")
    }
}

/// A held token and the time after which it is replaced; `None` where the endpoint named no
/// lifetime, so that it is kept until it is refused.
struct Held {
    token: Token,
    expires_at: Option<Instant>,
}

impl Held {
    fn is_fresh(&self, now: Instant) -> bool {
        self.expires_at
            .is_none_or(|expires_at| now + EXPIRY_MARGIN < expires_at)
    }
}

/// The tokens held, by the organisation they are for; the empty name is the one token that serves
/// every organisation.
fn store(held: &mut HashMap<String, Held>, now: Instant, key: String, token: Held) {
    held.retain(|_, held| held.is_fresh(now));
    held.insert(key, token);
}

enum Credentials {
    Secret {
        client_id: String,
        client_secret: String,
    },
    Assertion {
        client_id_template: String,
        key: EncodingKey,
        algorithm: Algorithm,
        key_id: Option<String>,
        issuer: String,
    },
}

/// A client of one token endpoint. Build one and share it by reference.
pub struct TokenClient {
    http: reqwest::Client,
    token_endpoint: String,
    audience: Option<String>,
    credentials: Credentials,
    held: Mutex<HashMap<String, Held>>,
    /// One lock per held-token key, so that concurrent misses for one organisation make one
    /// request; the lock over the map is never held across a request.
    flights: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

/// The claims of a client authentication assertion.
#[derive(Serialize)]
struct Claims<'a> {
    iss: &'a str,
    sub: &'a str,
    aud: &'a str,
    iat: u64,
    exp: u64,
    jti: String,
}

/// The members of a token response (RFC 6749 §5.1) the client reads.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
}

/// The member of an error response (RFC 6749 §5.2) the client reads.
#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
}

impl TokenClient {
    /// A client of `token_endpoint`, adding `audience` to each token request where given.
    ///
    /// # Errors
    ///
    /// [`InvalidClient`] where the HTTP client cannot be built, where the key is not a PEM PKCS#8
    /// RSA or P-256 private key, or where the client id template has no
    /// [`ORGANISATION_PLACEHOLDER`].
    pub fn new(
        token_endpoint: &str,
        audience: Option<String>,
        authentication: Authentication,
    ) -> Result<Self, InvalidClient> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| InvalidClient(error.to_string()))?;
        let credentials = match authentication {
            Authentication::ClientSecret {
                client_id,
                client_secret,
            } => Credentials::Secret {
                client_id,
                client_secret,
            },
            Authentication::PrivateKeyJwt {
                client_id_template,
                key_pem,
                key_id,
                issuer,
            } => {
                if !client_id_template.contains(ORGANISATION_PLACEHOLDER) {
                    return Err(InvalidClient(format!(
                        "the client id template has no {ORGANISATION_PLACEHOLDER}"
                    )));
                }
                let (key, algorithm) = signing_key(&key_pem)?;
                Credentials::Assertion {
                    client_id_template,
                    key,
                    algorithm,
                    key_id,
                    issuer,
                }
            }
        };
        Ok(Self {
            http,
            token_endpoint: token_endpoint.to_owned(),
            audience,
            credentials,
            held: Mutex::new(HashMap::new()),
            flights: Mutex::new(HashMap::new()),
        })
    }

    /// Whether a token is for one organisation, so that a caller deciding for several
    /// organisations asks for one token each; false where one token serves every organisation.
    #[must_use]
    pub const fn binds_organisation(&self) -> bool {
        matches!(self.credentials, Credentials::Assertion { .. })
    }

    /// The token for `organisation`: the held one while it is fresh, and a newly obtained one
    /// otherwise. Concurrent calls that find none fresh share one request.
    ///
    /// # Errors
    ///
    /// [`TokenError::UnauthorizedClient`] where the endpoint refuses the client for the
    /// organisation, and [`TokenError::Failed`] for any other failure.
    pub async fn token(&self, organisation: &str) -> Result<Token, TokenError> {
        let key = self.key(organisation);
        if let Some(token) = self.fresh(&key, None) {
            return Ok(token);
        }
        let flight = self.flight(&key);
        let _turn = flight.lock().await;
        if let Some(token) = self.fresh(&key, None) {
            return Ok(token);
        }
        self.fetch(organisation, key).await
    }

    /// A newly obtained token for `organisation`, which replaces the held one; for a caller whose
    /// token `rejected` the resource server refused. Where another caller has already replaced
    /// `rejected`, that token is returned and no request is made.
    ///
    /// # Errors
    ///
    /// As [`TokenClient::token`].
    pub async fn renewed(&self, organisation: &str, rejected: &Token) -> Result<Token, TokenError> {
        let key = self.key(organisation);
        let flight = self.flight(&key);
        let _turn = flight.lock().await;
        if let Some(token) = self.fresh(&key, Some(rejected)) {
            return Ok(token);
        }
        self.fetch(organisation, key).await
    }

    /// The token held under `key` while it is fresh and is not `except`.
    fn fresh(&self, key: &str, except: Option<&Token>) -> Option<Token> {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(key)
            .filter(|held| held.is_fresh(Instant::now()))
            .map(|held| held.token.clone())
            .filter(|token| Some(token) != except)
    }

    fn flight(&self, key: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut flights = self.flights.lock().unwrap_or_else(PoisonError::into_inner);
        Arc::clone(flights.entry(key.to_owned()).or_default())
    }

    /// Obtains a token and holds it under `key`.
    async fn fetch(&self, organisation: &str, key: String) -> Result<Token, TokenError> {
        let (token, expires_in) = self.obtain(organisation).await?;
        let now = Instant::now();
        let held = Held {
            token: token.clone(),
            expires_at: expires_in.map(|seconds| now + Duration::from_secs(seconds)),
        };
        store(
            &mut self.held.lock().unwrap_or_else(PoisonError::into_inner),
            now,
            key,
            held,
        );
        Ok(token)
    }

    fn key(&self, organisation: &str) -> String {
        if self.binds_organisation() {
            organisation.to_owned()
        } else {
            String::new()
        }
    }

    async fn obtain(&self, organisation: &str) -> Result<(Token, Option<u64>), TokenError> {
        let mut form = vec![("grant_type", "client_credentials".to_owned())];
        let mut request = self.http.post(&self.token_endpoint);
        match &self.credentials {
            Credentials::Secret {
                client_id,
                client_secret,
            } => {
                request = request.basic_auth(
                    form_urlencode(client_id),
                    Some(form_urlencode(client_secret)),
                );
            }
            Credentials::Assertion {
                client_id_template,
                key,
                algorithm,
                key_id,
                issuer,
            } => {
                let client_id = client_id_template.replace(ORGANISATION_PLACEHOLDER, organisation);
                let assertion =
                    sign_assertion(key, *algorithm, key_id.as_deref(), issuer, &client_id)?;
                form.push(("client_id", client_id));
                form.push(("client_assertion_type", CLIENT_ASSERTION_TYPE.to_owned()));
                form.push(("client_assertion", assertion));
            }
        }
        if let Some(audience) = &self.audience {
            form.push(("audience", audience.clone()));
        }
        let response = request.form(&form).send().await.map_err(failed)?;
        let status = response.status();
        if status == StatusCode::BAD_REQUEST {
            let refused: Option<ErrorResponse> = response.json().await.ok();
            return Err(match refused {
                Some(refused) if refused.error == "unauthorized_client" => {
                    TokenError::UnauthorizedClient
                }
                Some(refused) => TokenError::Failed(format!(
                    "the token endpoint refused the request: {}",
                    refused.error
                )),
                None => TokenError::Failed(format!("the token endpoint answered {status}")),
            });
        }
        let response: TokenResponse = response
            .error_for_status()
            .map_err(failed)?
            .json()
            .await
            .map_err(failed)?;
        if !response.token_type.eq_ignore_ascii_case("bearer") {
            return Err(TokenError::Failed(
                "the token endpoint issued a token that is not a bearer token".to_owned(),
            ));
        }
        Ok((Token(response.access_token), response.expires_in))
    }
}

/// The assertion by which the client `client_id` authenticates, signed with `key`.
fn sign_assertion(
    key: &EncodingKey,
    algorithm: Algorithm,
    key_id: Option<&str>,
    issuer: &str,
    client_id: &str,
) -> Result<String, TokenError> {
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(failed)?
        .as_secs();
    let mut header = Header::new(algorithm);
    header.typ = Some(ASSERTION_TYPE.to_owned());
    header.kid = key_id.map(str::to_owned);
    let claims = Claims {
        iss: client_id,
        sub: client_id,
        aud: issuer,
        iat: issued_at,
        exp: issued_at + ASSERTION_LIFETIME.as_secs(),
        jti: jti()?,
    };
    jsonwebtoken::encode(&header, &claims, key).map_err(failed)
}

/// The signing key of `pem` and the algorithm that follows it, found by signing once.
fn signing_key(pem: &[u8]) -> Result<(EncodingKey, Algorithm), InvalidClient> {
    let candidates = [
        (EncodingKey::from_ec_pem(pem), Algorithm::ES256),
        (EncodingKey::from_rsa_pem(pem), Algorithm::RS256),
    ];
    for (key, algorithm) in candidates {
        if let Ok(key) = key
            && jsonwebtoken::encode(&Header::new(algorithm), &serde_json::json!({}), &key).is_ok()
        {
            return Ok((key, algorithm));
        }
    }
    Err(InvalidClient(
        "the key is not a PEM PKCS#8 private key, RSA or P-256".to_owned(),
    ))
}

/// A random identifier for one assertion, 128 bits in hexadecimal.
fn jti() -> Result<String, TokenError> {
    let mut bytes = [0_u8; 16];
    aws_lc_rs::rand::fill(&mut bytes)
        .map_err(|_| TokenError::Failed("no random bytes for the assertion".to_owned()))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn failed(error: impl fmt::Display) -> TokenError {
    TokenError::Failed(error.to_string())
}

/// `text` in the `application/x-www-form-urlencoded` byte serialisation, which RFC 6749 §2.3.1
/// applies to the client id and secret before they are joined for HTTP Basic.
fn form_urlencode(text: &str) -> String {
    use std::fmt::Write;

    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'*' | b'-' | b'.' | b'_' => encoded.push(char::from(byte)),
            b' ' => encoded.push('+'),
            _ if byte.is_ascii_alphanumeric() => encoded.push(char::from(byte)),
            // Writing to a String does not fail.
            _ => {
                let _ = write!(encoded, "%{byte:02X}");
            }
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(expires_in: Option<Duration>) -> Held {
        Held {
            token: Token("t".to_owned()),
            expires_at: expires_in.map(|lifetime| Instant::now() + lifetime),
        }
    }

    #[test]
    fn drops_expired_tokens_when_one_is_stored() {
        let mut tokens = HashMap::new();
        tokens.insert("old".to_owned(), held(Some(Duration::ZERO)));
        tokens.insert("near".to_owned(), held(Some(Duration::from_secs(2))));
        tokens.insert("fresh".to_owned(), held(Some(Duration::from_secs(600))));
        tokens.insert("unbounded".to_owned(), held(None));

        store(&mut tokens, Instant::now(), "new".to_owned(), held(None));

        let mut names: Vec<_> = tokens.keys().map(String::as_str).collect();
        names.sort_unstable();
        assert_eq!(names, ["fresh", "new", "unbounded"]);
    }

    fn client_with(key_pem: Vec<u8>) -> Result<TokenClient, InvalidClient> {
        TokenClient::new(
            "http://token.test/token",
            None,
            Authentication::PrivateKeyJwt {
                client_id_template: "grn:{organisation}/iam/service/codehub".to_owned(),
                key_pem,
                key_id: None,
                issuer: "https://garam.test".to_owned(),
            },
        )
    }

    /// `der` as a PEM `PRIVATE KEY` block.
    fn pem(der: &[u8]) -> Vec<u8> {
        use base64::Engine as _;

        let body = base64::engine::general_purpose::STANDARD.encode(der);
        format!("-----BEGIN PRIVATE KEY-----\n{body}\n-----END PRIVATE KEY-----\n").into_bytes()
    }

    #[test]
    fn refuses_a_key_that_is_not_pem_pkcs8() {
        let ec = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .unwrap();
        let der_without_pem = ec.as_ref().to_vec();

        assert!(matches!(
            client_with(b"not a key".to_vec()),
            Err(InvalidClient(_))
        ));
        // A key's bytes that are not PEM are refused too, and the same key as PEM is accepted.
        assert!(matches!(client_with(der_without_pem), Err(InvalidClient(_))));
        assert!(client_with(pem(ec.as_ref())).is_ok());
    }

    #[test]
    fn follows_the_algorithm_of_the_key() {
        use aws_lc_rs::encoding::{AsDer, Pkcs8V1Der};

        let rsa = aws_lc_rs::rsa::KeyPair::generate(aws_lc_rs::rsa::KeySize::Rsa2048).unwrap();
        let rsa = AsDer::<Pkcs8V1Der>::as_der(&rsa).unwrap();
        let ec = aws_lc_rs::signature::EcdsaKeyPair::generate_pkcs8(
            &aws_lc_rs::signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            &aws_lc_rs::rand::SystemRandom::new(),
        )
        .unwrap();

        let (_, algorithm) = signing_key(&pem(rsa.as_ref())).unwrap();
        assert_eq!(algorithm, Algorithm::RS256);
        let (_, algorithm) = signing_key(&pem(ec.as_ref())).unwrap();
        assert_eq!(algorithm, Algorithm::ES256);
    }

    #[test]
    fn debug_shows_no_token() {
        assert_eq!(format!("{:?}", Token("secret".to_owned())), "Token(..)");
    }
}
