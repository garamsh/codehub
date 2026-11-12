use std::fmt::Write;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Bound on each request to the token endpoint and to the decision point.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// A token is replaced when it is this close to its expiry.
const EXPIRY_MARGIN: Duration = Duration::from_secs(5);

/// Authorisation by an AuthZEN 1.0 decision point. A decision is one evaluation request, and
/// decisions on many resources are one evaluations request where the adapter is enabled for it
/// and the decision point's metadata advertises it, read on first use (ADR 0041); the bearer
/// token is held, the decision never.
pub struct AuthzenAuthorisationDecision {
    http: reqwest::Client,
    metadata_endpoint: String,
    evaluation_endpoint: String,
    /// `access_evaluations_endpoint` of the metadata once it has been read successfully, `None`
    /// inside where the metadata does not advertise it.
    evaluations_endpoint: Mutex<Option<Option<String>>>,
    /// Whether decisions on many resources may be one evaluations request (ADR 0041).
    access_evaluations: bool,
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

/// The member of an evaluations response (AuthZEN 1.0 §7.2) the adapter reads.
#[derive(Deserialize)]
struct Evaluations {
    evaluations: Vec<Evaluation>,
}

/// The member of the decision point's metadata (AuthZEN 1.0 §9) the adapter reads.
#[derive(Deserialize)]
struct Metadata {
    access_evaluations_endpoint: Option<String>,
}

impl From<Evaluation> for Decision {
    fn from(evaluation: Evaluation) -> Self {
        if evaluation.decision {
            Self::Allow
        } else {
            Self::Deny
        }
    }
}

/// What the decision point answered one request.
enum Answer<T> {
    Decided(T),
    Unauthorised,
}

impl AuthzenAuthorisationDecision {
    /// An adapter that evaluates at `<base_url>/access/v1/evaluation` and reads the decision
    /// point's metadata at `<base_url>/.well-known/authzen-configuration`, with a bearer token
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
        let base_url = base_url.trim_end_matches('/');
        Ok(Self {
            http,
            metadata_endpoint: format!("{base_url}/.well-known/authzen-configuration"),
            evaluation_endpoint: format!("{base_url}/access/v1/evaluation"),
            evaluations_endpoint: Mutex::new(None),
            access_evaluations: false,
            token_endpoint: token_endpoint.to_owned(),
            client_id: client_id.to_owned(),
            client_secret: client_secret.to_owned(),
            token: Mutex::new(None),
        })
    }

    /// The adapter, sending decisions on many resources as one evaluations request where
    /// `enabled` and the decision point's metadata advertises it, and one evaluation per resource
    /// otherwise, which is the default. Enable it only for a decision point that decides each
    /// evaluation by its own resource properties (ADR 0041).
    #[must_use]
    pub const fn with_access_evaluations(mut self, enabled: bool) -> Self {
        self.access_evaluations = enabled;
        self
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

    /// The evaluations endpoint the metadata advertises, or none. The metadata is read on first
    /// use and kept once read; a failed read is tried again by the next call.
    async fn evaluations_endpoint(&self) -> Result<Option<String>, AuthorisationError> {
        let held = self
            .evaluations_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(endpoint) = held {
            return Ok(endpoint);
        }
        let metadata: Metadata = self
            .http
            .get(&self.metadata_endpoint)
            .send()
            .await
            .map_err(backend)?
            .error_for_status()
            .map_err(backend)?
            .json()
            .await
            .map_err(backend)?;
        *self
            .evaluations_endpoint
            .lock()
            .unwrap_or_else(PoisonError::into_inner) =
            Some(metadata.access_evaluations_endpoint.clone());
        Ok(metadata.access_evaluations_endpoint)
    }

    async fn post<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        bearer: &str,
        body: &Value,
    ) -> Result<Answer<T>, AuthorisationError> {
        let response = self
            .http
            .post(endpoint)
            .bearer_auth(bearer)
            .json(body)
            .send()
            .await
            .map_err(backend)?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(Answer::Unauthorised);
        }
        let answer = response
            .error_for_status()
            .map_err(backend)?
            .json()
            .await
            .map_err(backend)?;
        Ok(Answer::Decided(answer))
    }

    /// The answer to `body` posted to `endpoint`; where the token is refused, another is
    /// obtained and the request is sent once more.
    async fn ask<T: DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &Value,
    ) -> Result<T, AuthorisationError> {
        let bearer = self.bearer(false).await?;
        if let Answer::Decided(answer) = self.post(endpoint, &bearer, body).await? {
            return Ok(answer);
        }
        let bearer = self.bearer(true).await?;
        match self.post(endpoint, &bearer, body).await? {
            Answer::Decided(answer) => Ok(answer),
            Answer::Unauthorised => Err(AuthorisationError::Backend(
                "the decision point refused a newly obtained bearer token".to_owned(),
            )),
        }
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
            "subject": subject_value(subject),
            "action": {"name": action_name(action)},
            "resource": resource_value(resource),
        });
        let evaluation: Evaluation = self.ask(&self.evaluation_endpoint, &body).await?;
        Ok(evaluation.into())
    }

    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        if resources.is_empty() {
            return Ok(Vec::new());
        }
        let endpoint = if self.access_evaluations {
            self.evaluations_endpoint().await?
        } else {
            None
        };
        let Some(endpoint) = endpoint else {
            let mut decisions = Vec::with_capacity(resources.len());
            for resource in resources {
                decisions.push(self.decide(subject, action, resource).await?);
            }
            return Ok(decisions);
        };
        let body = json!({
            "subject": subject_value(subject),
            "action": {"name": action_name(action)},
            "evaluations": resources
                .iter()
                .map(|resource| json!({"resource": resource_value(resource)}))
                .collect::<Vec<_>>(),
        });
        let answer: Evaluations = self.ask(&endpoint, &body).await?;
        if answer.evaluations.len() != resources.len() {
            return Err(AuthorisationError::Backend(format!(
                "the decision point answered {} decisions for {} resources",
                answer.evaluations.len(),
                resources.len()
            )));
        }
        Ok(answer.evaluations.into_iter().map(Decision::from).collect())
    }
}

fn subject_value(subject: &Principal) -> Value {
    json!({
        "type": "principal",
        "id": subject.subject(),
        "properties": {"issuer": subject.issuer()},
    })
}

const fn action_name(action: Action) -> &'static str {
    match action {
        Action::CreateOrganisation => "create_organisation",
        Action::CreateRepository => "create_repository",
        Action::AdministerMembers => "administer_members",
        Action::AdministerRunners => "administer_runners",
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
