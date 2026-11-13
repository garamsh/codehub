//! The directory-change port over garam's `ApplyEntityChanges`: see the crate documentation for
//! the request and the answer.

use std::sync::Arc;
use std::time::Duration;

use app::port::driven::directory_changes::{
    ChangeKind, DirectoryChanges, OrganisationChange, RepositoryChange,
};
use oauth_client::TokenClient;
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};

/// Bound on each request to garam. One delivery is at most two token requests and two posts, so it
/// takes at most four of these; the relay's lease is longer than that.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Delivers repository changes to garam.
pub struct GaramDirectoryChanges {
    http: reqwest::Client,
    endpoint: String,
    tokens: Arc<TokenClient>,
}

/// The members of garam's answer the adapter reads.
#[derive(Deserialize)]
struct Applied {
    #[serde(default)]
    written: u32,
    #[serde(default)]
    dropped: u32,
    #[serde(default)]
    refusals: Vec<Value>,
}

/// What garam answered one request.
enum Answer {
    Applied(Applied),
    Unauthorised,
}

impl GaramDirectoryChanges {
    /// An adapter that delivers to `<base_url>/garam/policy/v1/entity-changes` with the bearer
    /// tokens of `tokens`.
    ///
    /// # Errors
    ///
    /// A description where the HTTP client cannot be built.
    pub fn new(base_url: &str, tokens: Arc<TokenClient>) -> Result<Self, String> {
        let url = reqwest::Url::parse(base_url)
            .map_err(|error| format!("{base_url:?} is not a URL: {error}"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(format!("{base_url:?} is not an http or https URL"));
        }
        let http = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| error.to_string())?;
        let base_url = base_url.trim_end_matches('/');
        Ok(Self {
            http,
            endpoint: format!("{base_url}/garam/policy/v1/entity-changes"),
            tokens,
        })
    }

    async fn post(&self, bearer: &str, body: &Value) -> Result<Answer, String> {
        let response = self
            .http
            .post(&self.endpoint)
            .bearer_auth(bearer)
            .json(body)
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if response.status() == StatusCode::UNAUTHORIZED {
            return Ok(Answer::Unauthorised);
        }
        let status = response.status();
        if !status.is_success() {
            return Err(format!("garam answered {status}"));
        }
        response
            .json()
            .await
            .map(Answer::Applied)
            .map_err(|error| error.to_string())
    }

    /// The answer to `body` sent with `organisation`'s token; where the token is refused, another
    /// is obtained and the request is sent once more.
    async fn send(&self, organisation: &str, body: &Value) -> Result<Applied, String> {
        let token = self
            .tokens
            .token(organisation)
            .await
            .map_err(|error| error.to_string())?;
        if let Answer::Applied(applied) = self.post(token.as_str(), body).await? {
            return Ok(applied);
        }
        let token = self
            .tokens
            .renewed(organisation, &token)
            .await
            .map_err(|error| error.to_string())?;
        match self.post(token.as_str(), body).await? {
            Answer::Applied(applied) => Ok(applied),
            Answer::Unauthorised => Err("garam refused a newly obtained bearer token".to_owned()),
        }
    }
}

/// An entity of garam's `grn` type.
fn entity(id: &str) -> Value {
    json!({"type": "grn", "id": id})
}

/// The body of the request that delivers `change`: one change, whose sequence is the string
/// protobuf's JSON form gives an `int64`.
fn body(change: &RepositoryChange) -> Value {
    let organisation = change.organisation.as_str();
    let id = format!("grn:{organisation}/codehub/repo/{}", change.repository);
    let mut entry = json!({"entity": entity(&id), "sequence": change.sequence.to_string()});
    let operation = match change.kind {
        ChangeKind::Created => {
            let org = entity(&format!("grn:root/iam/org/{organisation}"));
            ("upsert", json!({"org": org, "parents": [org]}))
        }
        ChangeKind::Deleted => ("delete", json!({})),
    };
    if let Some(members) = entry.as_object_mut() {
        members.insert(operation.0.to_owned(), operation.1);
    }
    json!({"changes": [entry]})
}

impl DirectoryChanges for GaramDirectoryChanges {
    async fn repository_changed(&self, change: &RepositoryChange) -> bool {
        match self.send(change.organisation.as_str(), &body(change)).await {
            Ok(applied)
                if applied.refusals.is_empty()
                    && applied.written.checked_add(applied.dropped) == Some(1) =>
            {
                true
            }
            Ok(applied) => {
                eprintln!(
                    "garam did not accept repository {} ({}/{}) sequence {}: written {}, dropped {}, refusals {:?}",
                    change.repository,
                    change.organisation,
                    change.name,
                    change.sequence,
                    applied.written,
                    applied.dropped,
                    applied.refusals
                );
                false
            }
            Err(error) => {
                eprintln!(
                    "delivery to garam failed for repository {} ({}/{}) sequence {}: {error}",
                    change.repository, change.organisation, change.name, change.sequence
                );
                false
            }
        }
    }

    async fn organisation_changed(&self, _change: &OrganisationChange) {}
}
