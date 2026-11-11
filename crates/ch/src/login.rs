//! The RFC 8628 device authorization grant against the issuer's discovery document, and the
//! principal a token names.

use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use reqwest::Client;
use serde::Deserialize;

use crate::api::encode;
use crate::failure::Failure;
use crate::wire::Principal;

/// The grant type of the token request (RFC 8628, section 3.4).
const GRANT_TYPE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// How long to wait between polls where the issuer names no interval (RFC 8628, section 3.2).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// How much longer to wait after `slow_down` (RFC 8628, section 3.5).
const SLOW_DOWN: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Discovery {
    device_authorization_endpoint: Option<String>,
    token_endpoint: String,
}

#[derive(Deserialize)]
struct DeviceAuthorization {
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: Option<u64>,
}

#[derive(Deserialize)]
struct TokenAnswer {
    access_token: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

/// What a token answer means for the poll.
#[derive(Debug, PartialEq, Eq)]
enum Step {
    /// The user approved.
    Done(String),
    /// Ask again after this long.
    Retry(Duration),
    /// The grant failed.
    Fail(String),
}

/// What to do after `answer`, polling every `interval`.
fn step(interval: Duration, answer: TokenAnswer) -> Step {
    match (answer.access_token, answer.error) {
        (Some(token), _) => Step::Done(token),
        (None, Some(error)) => match error.as_str() {
            "authorization_pending" => Step::Retry(interval),
            "slow_down" => Step::Retry(interval + SLOW_DOWN),
            _ => Step::Fail(match answer.error_description {
                Some(description) => format!("login failed: {error}: {description}"),
                None => format!("login failed: {error}"),
            }),
        },
        (None, None) => {
            Step::Fail("the token endpoint answered neither a token nor an error".to_owned())
        }
    }
}

/// A device authorization the user has yet to approve.
pub struct DeviceFlow {
    token_endpoint: String,
    client_id: String,
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    interval: Duration,
    deadline: Instant,
}

/// Asks the issuer's device authorization endpoint for a code.
///
/// # Errors
///
/// Fails where the issuer cannot be reached, advertises no device authorization endpoint, or
/// answers in a form that is not RFC 8628's.
pub async fn start(http: &Client, issuer: &str, client_id: &str) -> Result<DeviceFlow, Failure> {
    let discovery_url = format!(
        "{}/.well-known/openid-configuration",
        issuer.trim_end_matches('/')
    );
    let discovery: Discovery = http
        .get(discovery_url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let endpoint = discovery.device_authorization_endpoint.ok_or_else(|| {
        Failure::Other("the issuer advertises no device authorization endpoint".to_owned())
    })?;
    let authorization: DeviceAuthorization = http
        .post(endpoint)
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("client_id={}&scope=openid", encode(client_id)))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(DeviceFlow {
        token_endpoint: discovery.token_endpoint,
        client_id: client_id.to_owned(),
        device_code: authorization.device_code,
        user_code: authorization.user_code,
        verification_uri: authorization.verification_uri,
        verification_uri_complete: authorization.verification_uri_complete,
        interval: authorization
            .interval
            .map_or(DEFAULT_INTERVAL, Duration::from_secs),
        deadline: Instant::now() + Duration::from_secs(authorization.expires_in),
    })
}

impl DeviceFlow {
    /// What to tell the user: where to go and the code to enter.
    #[must_use]
    pub fn prompt(&self) -> String {
        let mut prompt = format!(
            "To log in, open {} and enter the code {}",
            self.verification_uri, self.user_code
        );
        if let Some(complete) = &self.verification_uri_complete {
            prompt.push_str(&format!("\nOr open {complete}"));
        }
        prompt
    }

    /// Polls the token endpoint until the user approves, and returns the access token.
    ///
    /// # Errors
    ///
    /// Fails where the user denies, the code expires or the endpoint cannot be reached.
    pub async fn poll(self, http: &Client) -> Result<String, Failure> {
        let body = format!(
            "grant_type={}&device_code={}&client_id={}",
            encode(GRANT_TYPE),
            encode(&self.device_code),
            encode(&self.client_id)
        );
        let mut interval = self.interval;
        loop {
            if Instant::now() + interval > self.deadline {
                return Err(Failure::Other("login failed: the code expired".to_owned()));
            }
            tokio::time::sleep(interval).await;
            let answer: TokenAnswer = http
                .post(&self.token_endpoint)
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body.clone())
                .send()
                .await?
                .json()
                .await?;
            match step(interval, answer) {
                Step::Done(token) => return Ok(token),
                Step::Retry(next) => interval = next,
                Step::Fail(message) => return Err(Failure::Other(message)),
            }
        }
    }
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
}

/// The issuer and subject `token` names, read from its payload without verifying it. Only the
/// server's verification means anything; this is for display and for defaults.
///
/// # Errors
///
/// Fails where `token` is not a JWT with `iss` and `sub` claims.
pub fn principal(token: &str) -> Result<Principal, Failure> {
    let claims: Claims = token
        .split('.')
        .nth(1)
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|payload| serde_json::from_slice(&payload).ok())
        .ok_or_else(|| {
            Failure::Other("the token is not a JWT with iss and sub claims".to_owned())
        })?;
    Ok(Principal {
        issuer: claims.iss,
        subject: claims.sub,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(token: Option<&str>, error: Option<&str>) -> TokenAnswer {
        TokenAnswer {
            access_token: token.map(str::to_owned),
            error: error.map(str::to_owned),
            error_description: None,
        }
    }

    const INTERVAL: Duration = Duration::from_secs(5);

    #[test]
    fn finishes_with_the_token() {
        assert_eq!(
            step(INTERVAL, answer(Some("t"), None)),
            Step::Done("t".to_owned())
        );
    }

    #[test]
    fn keeps_the_interval_while_authorization_is_pending() {
        assert_eq!(
            step(INTERVAL, answer(None, Some("authorization_pending"))),
            Step::Retry(INTERVAL)
        );
    }

    #[test]
    fn waits_five_seconds_longer_after_slow_down() {
        assert_eq!(
            step(INTERVAL, answer(None, Some("slow_down"))),
            Step::Retry(Duration::from_secs(10))
        );
    }

    #[test]
    fn fails_on_any_other_error() {
        for error in ["access_denied", "expired_token"] {
            assert_eq!(
                step(INTERVAL, answer(None, Some(error))),
                Step::Fail(format!("login failed: {error}"))
            );
        }
    }

    #[test]
    fn reads_the_issuer_and_subject_of_a_jwt() {
        // {"alg":"none"}.{"iss":"https://id.test","sub":"ada"}.
        let token = "eyJhbGciOiJub25lIn0.eyJpc3MiOiJodHRwczovL2lkLnRlc3QiLCJzdWIiOiJhZGEifQ.sig";
        let principal = principal(token).unwrap();
        assert_eq!(principal.issuer, "https://id.test");
        assert_eq!(principal.subject, "ada");
    }

    #[test]
    fn rejects_a_token_that_is_not_a_jwt() {
        for token in ["opaque", "a.!!!.c", "a.e30.c"] {
            assert!(principal(token).is_err(), "{token}");
        }
    }
}
