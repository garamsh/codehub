use std::fmt::Write;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

/// Bound on each request to the token endpoint and to the decision point.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A token is replaced when it is this close to its expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(5);

/// Authorisation by an AuthZEN 1.0 decision point. Every decision is one evaluation request;
/// the bearer token is held, the decision never.
pub struct AuthzenAuthorisationDecision {
    http: reqwest::Client,
    evaluation_endpoint: String,
    token_endpoint: String,
    client_id: String,
    client_secret: String,
    token: Mutex<Option<Token>>,
}

/// A bearer token and the time after which it is replaced; `None` where the endpoint named no
/// lifetime, so that it is kept until the decision point refuses it.
#[derive(Clone)]
struct Token {
    value: String,
    expires_at: Option<Instant>,
}

impl Token {
    fn is_fresh(&self) -> bool {
        self.expires_at
            .is_none_or(|expires_at| Instant::now() + EXPIRY_MARGIN < expires_at)
    }
}

/// The members of a token response (RFC 6749 §5.1) the adapter reads.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
}

/// The member of an evaluation response (AuthZEN 1.0 §6.2) the adapter reads.
#[derive(Deserialize)]
struct Evaluation {
    decision: bool,
}

/// What the decision point answered one request.
enum Answer {
    Decided(Decision),
    Unauthorised,
}

impl AuthzenAuthorisationDecision {
    /// An adapter that evaluates at `<base_url>/access/v1/evaluation`, with a bearer token
    /// obtained from `token_endpoint` as the client `client_id` with `client_secret`.
    ///
    /// # Errors
    ///
    /// [`AuthorisationError::Backend`] where the HTTP client cannot be built.
    pub fn new(
        base_url: &str,
        token_endpoint: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Result<Self, AuthorisationError> {
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(backend)?;
        Ok(Self {
            http,
            evaluation_endpoint: format!("{}/access/v1/evaluation", base_url.trim_end_matches('/')),
            token_endpoint: token_endpoint.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            token: Mutex::new(None),
        })
    }

    /// The held token while it is fresh, and a newly obtained one otherwise or where `renew`.
    async fn bearer(&self, renew: bool) -> Result<String, AuthorisationError> {
        let held = self
            .token
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if !renew && let Some(token) = held.filter(Token::is_fresh) {
            return Ok(token.value);
        }
        let token = self.obtain_token().await?;
        let value = token.value.clone();
        *self.token.lock().unwrap_or_else(PoisonError::into_inner) = Some(token);
        Ok(value)
    }

    async fn obtain_token(&self) -> Result<Token, AuthorisationError> {
        let response: TokenResponse = self
            .http
            .post(&self.token_endpoint)
            .basic_auth(
                form_urlencode(&self.client_id),
                Some(form_urlencode(&self.client_secret)),
            )
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(backend)?
            .error_for_status()
            .map_err(backend)?
            .json()
            .await
            .map_err(backend)?;
        if !response.token_type.eq_ignore_ascii_case("bearer") {
            return Err(AuthorisationError::Backend(
                "the token endpoint issued a token that is not a bearer token".to_owned(),
            ));
        }
        Ok(Token {
            value: response.access_token,
            expires_at: response
                .expires_in
                .map(|seconds| Instant::now() + Duration::from_secs(seconds)),
        })
    }

    async fn evaluate(&self, bearer: &str, body: &Value) -> Result<Answer, AuthorisationError> {
        let response = self
            .http
            .post(&self.evaluation_endpoint)
            .bearer_auth(bearer)
            .json(body)
            .send()
            .await
            .map_err(backend)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(Answer::Unauthorised);
        }
        let evaluation: Evaluation = response
            .error_for_status()
            .map_err(backend)?
            .json()
            .await
            .map_err(backend)?;
        Ok(Answer::Decided(if evaluation.decision {
            Decision::Allow
        } else {
            Decision::Deny
        }))
    }
}

impl AuthorisationDecision for AuthzenAuthorisationDecision {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        let body = json!({
            "subject": {
                "type": "principal",
                "id": subject.subject(),
                "properties": {"issuer": subject.issuer()},
            },
            "action": {"name": action_name(action)},
            "resource": resource_value(resource),
        });
        let bearer = self.bearer(false).await?;
        if let Answer::Decided(decision) = self.evaluate(&bearer, &body).await? {
            return Ok(decision);
        }
        // The token was refused: obtain another and ask once more.
        let bearer = self.bearer(true).await?;
        match self.evaluate(&bearer, &body).await? {
            Answer::Decided(decision) => Ok(decision),
            Answer::Unauthorised => Err(AuthorisationError::Backend(
                "the decision point refused a newly obtained bearer token".to_owned(),
            )),
        }
    }
}

const fn action_name(action: Action) -> &'static str {
    match action {
        Action::CreateOrganisation => "create_organisation",
        Action::CreateRepository => "create_repository",
        Action::AdministerMembers => "administer_members",
        Action::Read => "read",
        Action::Write => "write",
    }
}

fn resource_value(resource: &Resource) -> Value {
    match resource {
        Resource::Organisation(organisation) => {
            json!({"type": "organisation", "id": organisation.as_str()})
        }
        Resource::Repository {
            organisation, name, ..
        } => json!({
            "type": "repository",
            "id": format!("{organisation}/{name}"),
            "properties": {"organisation": organisation.as_str(), "repository": name.as_str()},
        }),
        Resource::Path {
            organisation,
            name,
            path,
            ..
        } => json!({
            "type": "repository",
            "id": format!("{organisation}/{name}"),
            "properties": {
                "organisation": organisation.as_str(),
                "repository": name.as_str(),
                "path": path.to_string(),
            },
        }),
    }
}

fn backend(error: reqwest::Error) -> AuthorisationError {
    AuthorisationError::Backend(error.to_string())
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
