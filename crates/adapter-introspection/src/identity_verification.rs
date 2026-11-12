use std::fmt::Write;
use std::time::Duration;

use app::domain::credential::Credential;
use app::domain::principal::Principal;
use app::port::driven::identity_verification::{IdentityError, IdentityVerification};
use serde::Deserialize;

/// Bound on each request to the introspection endpoint.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Identity verification by RFC 7662 token introspection. Every verification is one request to
/// the endpoint, authenticated as the configured client; nothing is cached.
pub struct IntrospectionIdentityVerification {
    http: reqwest::Client,
    endpoint: String,
    client_id: String,
    client_secret: String,
    issuer: String,
    audience: String,
}

/// The members of an introspection response (RFC 7662 §2.2) the verification reads.
#[derive(Deserialize)]
struct Response {
    active: bool,
    iss: Option<String>,
    sub: Option<String>,
    aud: Option<Audience>,
}

/// `aud` is a string or an array of strings (RFC 7519 §4.1.3).
#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

impl Audience {
    fn contains(&self, audience: &str) -> bool {
        match self {
            Self::One(one) => one == audience,
            Self::Many(many) => many.iter().any(|candidate| candidate == audience),
        }
    }
}

impl IntrospectionIdentityVerification {
    /// A verifier that posts tokens to `endpoint`, authenticating with `client_id` and
    /// `client_secret`, and accepts those issued for `audience`. A response without `iss` names
    /// `issuer`.
    ///
    /// # Errors
    ///
    /// [`IdentityError::Backend`] where the HTTP client cannot be built.
    pub fn new(
        endpoint: &str,
        client_id: &str,
        client_secret: &str,
        issuer: &str,
        audience: &str,
    ) -> Result<Self, IdentityError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(backend)?;
        Ok(Self {
            http,
            endpoint: endpoint.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            issuer: issuer.to_owned(),
            audience: audience.to_owned(),
        })
    }
}

impl IdentityVerification for IntrospectionIdentityVerification {
    async fn verify(&self, credential: &Credential) -> Result<Principal, IdentityError> {
        let response = self
            .http
            .post(&self.endpoint)
            .basic_auth(
                form_urlencode(&self.client_id),
                Some(form_urlencode(&self.client_secret)),
            )
            .form(&[("token", credential.as_str())])
            .send()
            .await
            .map_err(backend)?
            .error_for_status()
            .map_err(backend)?;
        let response: Response = response.json().await.map_err(backend)?;
        if !response.active {
            return Err(IdentityError::Rejected);
        }
        if !response
            .aud
            .is_some_and(|audience| audience.contains(&self.audience))
        {
            return Err(IdentityError::Rejected);
        }
        let subject = response.sub.ok_or(IdentityError::Rejected)?;
        let issuer = response.iss.as_deref().unwrap_or(&self.issuer);
        Principal::new(issuer, &subject).map_err(|_| IdentityError::Rejected)
    }
}

fn backend(error: reqwest::Error) -> IdentityError {
    IdentityError::Backend(error.to_string())
}

/// `text` in the `application/x-www-form-urlencoded` byte serialisation, which RFC 6749 §2.3.1
/// applies to the client id and secret before they are joined for HTTP Basic.
fn form_urlencode(text: &str) -> String {
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
