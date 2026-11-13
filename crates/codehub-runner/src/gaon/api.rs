//! The part of gaon's resource API the driver uses: create, read, list and delete of MicroVMs
//! and Secrets, each request carrying a token read from its file just before it is sent.

use std::fmt;
use std::time::Duration;

use serde_json::Value;

use crate::config::RUNNER_LABEL;
use crate::token::TokenSource;

/// How long one request may take.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A kind of object the driver creates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// A MicroVM, which runs a job.
    MicroVm,
    /// A Secret, which holds a job's description.
    Secret,
}

impl Kind {
    const fn plural(self) -> &'static str {
        match self {
            Self::MicroVm => "microvms",
            Self::Secret => "secrets",
        }
    }
}

/// Why a request failed. It never holds the token.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ApiError {
    /// gaon answered with a status other than success.
    Status {
        /// The HTTP status.
        code: u16,
        /// The `reason` of gaon's error body, or empty.
        reason: String,
    },
    /// The request was not sent or its answer not read.
    Transport(String),
}

impl ApiError {
    /// Whether gaon answered with `code`.
    #[must_use]
    pub fn is_status(&self, code: u16) -> bool {
        matches!(self, Self::Status { code: got, .. } if *got == code)
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Status { code, reason } if reason.is_empty() => write!(f, "gaon answered {code}"),
            Self::Status { code, reason } => write!(f, "gaon answered {code} {reason}"),
            Self::Transport(reason) => write!(f, "gaon could not be reached: {reason}"),
        }
    }
}

impl std::error::Error for ApiError {}

/// An object of a labelled list.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Listed {
    /// Its name.
    pub name: String,
    /// The version a delete must name.
    pub resource_version: String,
    /// Whether gaon is already deleting it.
    pub deleting: bool,
}

/// A MicroVM's phase and why it is not ready.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Phase {
    /// The phase, `Pending` where the status is not of the current binding.
    pub phase: String,
    /// The reason of the `Ready` condition, where it has one.
    pub reason: Option<String>,
}

impl Phase {
    /// Whether the MicroVM will not run again.
    #[must_use]
    pub fn is_final(&self) -> bool {
        self.phase == "Succeeded" || self.phase == "Failed"
    }
}

/// gaon's API as the runner's identity sees it.
pub struct Api {
    http: reqwest::Client,
    root: String,
    tokens: TokenSource,
    runner: String,
}

impl Api {
    /// A client of the API at `root`, which labels and selects by `runner`.
    #[must_use]
    pub fn new(http: reqwest::Client, root: &str, tokens: TokenSource, runner: String) -> Self {
        Self {
            http,
            root: root.trim_end_matches('/').to_owned(),
            tokens,
            runner,
        }
    }

    /// The metadata labels every object carries.
    #[must_use]
    pub fn labels(&self) -> Value {
        serde_json::json!({ RUNNER_LABEL: self.runner })
    }

    fn selector(&self) -> String {
        format!("{RUNNER_LABEL}={}", self.runner)
    }

    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let token = self
            .tokens
            .token()
            .await
            .map_err(|error| ApiError::Transport(error.to_string()))?;
        let response = request
            .bearer_auth(token)
            .timeout(REQUEST_TIMEOUT)
            .send()
            .await
            .map_err(|error| ApiError::Transport(error.without_url().to_string()))?;
        let status = response.status();
        let body: Value = response.json().await.unwrap_or(Value::Null);
        if status.is_success() {
            Ok(body)
        } else {
            let reason = body
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            Err(ApiError::Status {
                code: status.as_u16(),
                reason,
            })
        }
    }

    /// Creates the object `body` describes. An existing one is an error.
    ///
    /// # Errors
    ///
    /// An [`ApiError`] where gaon refuses or cannot be reached.
    pub async fn create(&self, kind: Kind, name: &str, body: &Value) -> Result<(), ApiError> {
        let url = format!("{}/{}/{name}", self.root, kind.plural());
        self.send(self.http.put(url).json(body)).await.map(drop)
    }

    /// The phase of the MicroVM `name`.
    ///
    /// # Errors
    ///
    /// An [`ApiError`] where gaon refuses or cannot be reached; the runner's identity is refused
    /// a MicroVM that is gone.
    pub async fn phase(&self, name: &str) -> Result<Phase, ApiError> {
        let url = format!("{}/microvms/{name}", self.root);
        let object = self.send(self.http.get(url)).await?;
        let epoch = |value: Option<&Value>| value.and_then(|v| v.get("bindingEpoch")).cloned();
        let status = object.get("status");
        let current = epoch(status) == epoch(object.get("spec"));
        let text = |value: Option<&Value>, key: &str| {
            value
                .and_then(|v| v.get(key))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let phase = text(status, "phase").filter(|_| current);
        let reason = status
            .and_then(|s| s.get("conditions"))
            .and_then(Value::as_array)
            .and_then(|all| {
                all.iter()
                    .find(|c| c.get("type").and_then(Value::as_str) == Some("Ready"))
            })
            .and_then(|c| text(Some(c), "reason"))
            .filter(|reason| !reason.is_empty());
        Ok(Phase {
            phase: phase.unwrap_or_else(|| "Pending".to_owned()),
            reason,
        })
    }

    /// The objects of `kind` that carry the runner's label.
    ///
    /// # Errors
    ///
    /// An [`ApiError`] where gaon refuses or cannot be reached.
    pub async fn list(&self, kind: Kind) -> Result<Vec<Listed>, ApiError> {
        let url = format!("{}/{}", self.root, kind.plural());
        let list = self
            .send(
                self.http
                    .get(url)
                    .query(&[("labelSelector", self.selector())]),
            )
            .await?;
        let items = list
            .get("items")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        Ok(items
            .iter()
            .filter_map(|item| {
                let meta = item.get("metadata")?;
                let text = |key: &str| meta.get(key).and_then(Value::as_str);
                Some(Listed {
                    name: text("name")?.to_owned(),
                    resource_version: text("resourceVersion")?.to_owned(),
                    deleting: text("deletionTimestamp").is_some_and(|t| !t.is_empty()),
                })
            })
            .collect())
    }

    /// Deletes `name` at `resource_version`.
    ///
    /// # Errors
    ///
    /// An [`ApiError`] where gaon refuses or cannot be reached. A missing object answers
    /// Forbidden, and a stale version Conflict.
    pub async fn delete(
        &self,
        kind: Kind,
        name: &str,
        resource_version: &str,
    ) -> Result<(), ApiError> {
        let url = format!("{}/{}/{name}", self.root, kind.plural());
        self.send(
            self.http
                .delete(url)
                .query(&[("resourceVersion", resource_version)]),
        )
        .await
        .map(drop)
    }
}
