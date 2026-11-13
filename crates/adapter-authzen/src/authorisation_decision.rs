use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use app::domain::path::Path;
use app::domain::principal::Principal;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use oauth_client::{TokenClient, TokenError};
use reqwest::StatusCode;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// Bound on each request to the decision point.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Authorisation by an AuthZEN 1.0 decision point. A decision is one evaluation request, and
/// decisions on many resources are one evaluations request where the adapter is enabled for it
/// and the decision point's metadata advertises it, read on first use (ADR 0041); the bearer
/// token is held by the token client, the decision never.
pub struct AuthzenAuthorisationDecision {
    http: reqwest::Client,
    metadata_endpoint: String,
    evaluation_endpoint: String,
    /// `access_evaluations_endpoint` of the metadata once it has been read successfully, `None`
    /// inside where the metadata does not advertise it.
    evaluations_endpoint: Mutex<Option<Option<String>>>,
    /// Whether decisions on many resources may be one evaluations request (ADR 0041).
    access_evaluations: bool,
    tokens: Arc<TokenClient>,
    profile: Profile,
}

/// The names the adapter sends: what a decision point is written against (ADR 0043).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Profile {
    /// The neutral names of ADR 0030: the type `principal` with the issuer as a property, the
    /// actions `create_organisation` to `write`, and a repository by its name.
    Plain,
    /// garam's contract: the type `grn`, a repository by its id, the `codehub:` actions, and a
    /// path as `context.path`.
    Grn,
}

/// The member of an evaluation response (AuthZEN 1.0 §6.2) the adapter reads.
#[derive(Deserialize)]
struct Evaluated {
    decision: bool,
}

/// The member of an evaluations response (AuthZEN 1.0 §7.2) the adapter reads.
#[derive(Deserialize)]
struct Evaluations {
    evaluations: Vec<Evaluated>,
}

/// The member of the decision point's metadata (AuthZEN 1.0 §9) the adapter reads.
#[derive(Deserialize)]
struct Metadata {
    access_evaluations_endpoint: Option<String>,
}

impl From<Evaluated> for Decision {
    fn from(evaluated: Evaluated) -> Self {
        if evaluated.decision {
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

/// What is asked of one resource: the resource as the profile names it, and the context that goes
/// with it.
struct Evaluation {
    resource: Value,
    context: Option<Value>,
}

impl Evaluation {
    /// The body of an evaluation request (AuthZEN 1.0 §6.1).
    fn request(&self, subject: &Value, action: &Value) -> Value {
        let mut body = json!({"subject": subject, "action": action, "resource": self.resource});
        if let (Some(context), Some(members)) = (&self.context, body.as_object_mut()) {
            members.insert("context".to_owned(), context.clone());
        }
        body
    }

    /// An entry of an evaluations request (AuthZEN 1.0 §7.1), whose context is its own.
    fn entry(&self) -> Value {
        let mut entry = json!({"resource": self.resource});
        if let (Some(context), Some(members)) = (&self.context, entry.as_object_mut()) {
            members.insert("context".to_owned(), context.clone());
        }
        entry
    }
}

impl AuthzenAuthorisationDecision {
    /// An adapter that evaluates at `<base_url>/access/v1/evaluation` and reads the decision
    /// point's metadata at `<base_url>/.well-known/authzen-configuration`, with the bearer tokens
    /// of `tokens` and the names of `profile`.
    ///
    /// # Errors
    ///
    /// [`AuthorisationError::Backend`] where the HTTP client cannot be built.
    pub fn new(
        base_url: &str,
        tokens: Arc<TokenClient>,
        profile: Profile,
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
            tokens,
            profile,
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

    /// The answer to `body` posted to `endpoint` with `organisation`'s token; where the token is
    /// refused, another is obtained and the request is sent once more. `None` where the token
    /// endpoint refuses the client for the organisation, which denies (ADR 0043).
    async fn ask<T: DeserializeOwned>(
        &self,
        organisation: &str,
        endpoint: &str,
        body: &Value,
    ) -> Result<Option<T>, AuthorisationError> {
        let token = match self.tokens.token(organisation).await {
            Ok(token) => token,
            Err(error) => return self.refusal(error),
        };
        if let Answer::Decided(answer) = self.post(endpoint, token.as_str(), body).await? {
            return Ok(Some(answer));
        }
        let token = match self.tokens.renewed(organisation).await {
            Ok(token) => token,
            Err(error) => return self.refusal(error),
        };
        match self.post(endpoint, token.as_str(), body).await? {
            Answer::Decided(answer) => Ok(Some(answer)),
            Answer::Unauthorised => Err(AuthorisationError::Backend(
                "the decision point refused a newly obtained bearer token".to_owned(),
            )),
        }
    }

    /// What a failure to obtain a token comes to: no answer where the endpoint refuses a client
    /// that is one per organisation as `unauthorized_client`, and a failure otherwise.
    fn refusal<T>(&self, error: TokenError) -> Result<Option<T>, AuthorisationError> {
        match error {
            TokenError::UnauthorizedClient if self.tokens.binds_organisation() => Ok(None),
            TokenError::UnauthorizedClient | TokenError::Failed(_) => {
                Err(AuthorisationError::Backend(error.to_string()))
            }
        }
    }

    /// The subject as the profile names it.
    fn subject_value(&self, subject: &Principal) -> Value {
        match self.profile {
            Profile::Plain => json!({
                "type": "principal",
                "id": subject.subject(),
                "properties": {"issuer": subject.issuer()},
            }),
            Profile::Grn => json!({"type": "grn", "id": subject.subject()}),
        }
    }

    /// What to ask for `action` on `resource`, or `None` where the path is not valid UTF-8, which
    /// is denied without asking (ADR 0043).
    fn evaluation(&self, action: Action, resource: &Resource) -> Option<Evaluation> {
        let path = match resource {
            Resource::Path { path, .. } => Some(utf8_path(path)?),
            Resource::Organisation(_) | Resource::Repository { .. } => None,
        };
        Some(match self.profile {
            Profile::Plain => Evaluation {
                resource: plain_resource(resource, path),
                context: None,
            },
            Profile::Grn => Evaluation {
                resource: grn_resource(resource),
                context: path
                    .filter(|_| matches!(action, Action::Read | Action::Write))
                    .map(|path| json!({"path": path})),
            },
        })
    }

    /// The decisions on `evaluations`, all for `organisation`, in order: one evaluations request
    /// where `batch` and the decision point offers it, one evaluation request each otherwise.
    /// `None` where the token endpoint refuses the client for the organisation.
    async fn decide_group(
        &self,
        subject: &Value,
        action: &Value,
        organisation: &str,
        evaluations: &[Evaluation],
        batch: bool,
    ) -> Result<Option<Vec<Decision>>, AuthorisationError> {
        let endpoint = if batch && self.access_evaluations {
            self.evaluations_endpoint().await?
        } else {
            None
        };
        let Some(endpoint) = endpoint else {
            let mut decisions = Vec::with_capacity(evaluations.len());
            for evaluation in evaluations {
                let body = evaluation.request(subject, action);
                let answer: Option<Evaluated> = self
                    .ask(organisation, &self.evaluation_endpoint, &body)
                    .await?;
                let Some(answer) = answer else {
                    return Ok(None);
                };
                decisions.push(answer.into());
            }
            return Ok(Some(decisions));
        };
        let body = json!({
            "subject": subject,
            "action": action,
            "evaluations": evaluations.iter().map(Evaluation::entry).collect::<Vec<_>>(),
        });
        let answer: Option<Evaluations> = self.ask(organisation, &endpoint, &body).await?;
        let Some(answer) = answer else {
            return Ok(None);
        };
        if answer.evaluations.len() != evaluations.len() {
            return Err(AuthorisationError::Backend(format!(
                "the decision point answered {} decisions for {} resources",
                answer.evaluations.len(),
                evaluations.len()
            )));
        }
        Ok(Some(
            answer.evaluations.into_iter().map(Decision::from).collect(),
        ))
    }

    /// The decisions on `resources`, in order. A resource denied without asking, or whose
    /// organisation's token is refused as `unauthorized_client`, is denied.
    async fn decide_resources(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
        batch: bool,
    ) -> Result<Vec<Decision>, AuthorisationError> {
        let action_value = json!({"name": action_name(self.profile, action)?});
        let subject = self.subject_value(subject);
        let mut decisions = vec![Decision::Deny; resources.len()];
        // The resources to ask about, grouped by the organisation whose token asks for them, in
        // the order the organisations first appear; where one token serves all, one group.
        let mut groups: Vec<(&str, Vec<usize>, Vec<Evaluation>)> = Vec::new();
        for (index, resource) in resources.iter().enumerate() {
            let Some(evaluation) = self.evaluation(action, resource) else {
                continue;
            };
            let organisation = if self.tokens.binds_organisation() {
                organisation_of(resource)
            } else {
                ""
            };
            match groups.iter_mut().find(|(name, ..)| *name == organisation) {
                Some((_, indexes, evaluations)) => {
                    indexes.push(index);
                    evaluations.push(evaluation);
                }
                None => groups.push((organisation, vec![index], vec![evaluation])),
            }
        }
        for (organisation, indexes, evaluations) in groups {
            let answers = self
                .decide_group(&subject, &action_value, organisation, &evaluations, batch)
                .await?;
            for (index, answer) in indexes.into_iter().zip(answers.into_iter().flatten()) {
                if let Some(decision) = decisions.get_mut(index) {
                    *decision = answer;
                }
            }
        }
        Ok(decisions)
    }
}

impl AuthorisationDecision for AuthzenAuthorisationDecision {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        let decisions = self
            .decide_resources(subject, action, std::slice::from_ref(resource), false)
            .await?;
        Ok(decisions.first().copied().unwrap_or(Decision::Deny))
    }

    async fn decide_all(
        &self,
        subject: &Principal,
        action: Action,
        resources: &[Resource],
    ) -> Result<Vec<Decision>, AuthorisationError> {
        self.decide_resources(subject, action, resources, true)
            .await
    }
}

/// The name of `action` in `profile`; the GRN profile has none for administering members, which
/// external mode never asks (ADR 0030, ADR 0043).
fn action_name(profile: Profile, action: Action) -> Result<&'static str, AuthorisationError> {
    Ok(match (profile, action) {
        (Profile::Plain, Action::CreateOrganisation) => "create_organisation",
        (Profile::Plain, Action::CreateRepository) => "create_repository",
        (Profile::Plain, Action::AdministerMembers) => "administer_members",
        (Profile::Plain, Action::AdministerRunners) => "administer_runners",
        (Profile::Plain, Action::Read) => "read",
        (Profile::Plain, Action::Write) => "write",
        (Profile::Grn, Action::CreateOrganisation) => "codehub:org.use",
        (Profile::Grn, Action::CreateRepository) => "codehub:repo.create",
        (Profile::Grn, Action::AdministerRunners) => "codehub:runner.administer",
        (Profile::Grn, Action::Read) => "codehub:repo.read",
        (Profile::Grn, Action::Write) => "codehub:repo.write",
        (Profile::Grn, Action::AdministerMembers) => {
            return Err(AuthorisationError::Backend(
                "the grn profile has no action for administering members".to_owned(),
            ));
        }
    })
}

/// The organisation a resource is in, by whose token it is asked about.
fn organisation_of(resource: &Resource) -> &str {
    match resource {
        Resource::Organisation(organisation)
        | Resource::Repository { organisation, .. }
        | Resource::Path { organisation, .. } => organisation.as_str(),
    }
}

/// The path's components joined by `/`, or `None` where one is not valid UTF-8.
fn utf8_path(path: &Path) -> Option<String> {
    let components = path
        .components()
        .iter()
        .map(|component| std::str::from_utf8(component.as_bytes()).ok())
        .collect::<Option<Vec<_>>>()?;
    Some(components.join("/"))
}

fn plain_resource(resource: &Resource, path: Option<String>) -> Value {
    match resource {
        Resource::Organisation(organisation) => {
            json!({"type": "organisation", "id": organisation.as_str()})
        }
        Resource::Repository {
            organisation, name, ..
        }
        | Resource::Path {
            organisation, name, ..
        } => {
            let mut properties = json!({
                "organisation": organisation.as_str(),
                "repository": name.as_str(),
            });
            if let (Some(path), Some(members)) = (path, properties.as_object_mut()) {
                members.insert("path".to_owned(), Value::String(path));
            }
            json!({
                "type": "repository",
                "id": format!("{organisation}/{name}"),
                "properties": properties,
            })
        }
    }
}

fn grn_resource(resource: &Resource) -> Value {
    let id = match resource {
        Resource::Organisation(organisation) => format!("grn:root/iam/org/{organisation}"),
        Resource::Repository {
            organisation,
            repository,
            ..
        }
        | Resource::Path {
            organisation,
            repository,
            ..
        } => format!("grn:{organisation}/codehub/repo/{repository}"),
    };
    json!({"type": "grn", "id": id})
}

fn backend(error: reqwest::Error) -> AuthorisationError {
    AuthorisationError::Backend(error.to_string())
}
