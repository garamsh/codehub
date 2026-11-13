//! The gaon adapter of the executor driver port: runs a job's command in a run-once MicroVM and
//! takes its end from the result the MicroVM posts back. ADR 0046 decides the steps.

pub mod api;
pub mod results;

use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use rustix::rand::{GetRandomFlags, getrandom};
use serde_json::{Value, json};
use tokio::time::Instant;

use crate::config::JobUser;
use crate::executor::{End, Execution, Executor, ExecutorError, Stop};
use crate::gaon::api::{Api, ApiError, Kind, Listed};
use crate::gaon::results::{Expectation, Results};
use crate::job_source::Job;
use crate::run_job::Description;

/// The key of the description in its Secret.
const DESCRIPTION_KEY: &str = "job.json";

/// Where the MicroVM finds the description.
const DESCRIPTION_PATH: &str = "/run/codehub-job.json";

/// The most a Secret holds, keys and values together, in bytes.
pub const DESCRIPTION_LIMIT: usize = 512 * 1024;

/// How often the driver reads a MicroVM's phase, and a labelled list while it waits for a delete.
const POLL: Duration = Duration::from_secs(2);

/// How long the driver waits for an object it deleted to leave the labelled list.
const GONE_WAIT: Duration = Duration::from_secs(60);

const API_VERSION: &str = "compute.gaon.garam.sh/v1alpha1";

/// The bytes the job's result secret is made of.
const SECRET_BYTES: usize = 32;

/// Runs jobs in MicroVMs on gaon.
pub struct GaonExecutor {
    api: Api,
    image: String,
    user: JobUser,
    result_url: String,
    results: Results,
    poll: Duration,
    gone_wait: Duration,
}

fn orchestrator(error: &ApiError) -> ExecutorError {
    ExecutorError::Orchestrator(error.to_string())
}

/// 32 random bytes as hex.
fn new_secret() -> Result<String, ExecutorError> {
    let mut bytes = [0_u8; SECRET_BYTES];
    let mut filled = 0;
    while filled < SECRET_BYTES {
        let rest = bytes.get_mut(filled..).unwrap_or_default();
        filled += getrandom(rest, GetRandomFlags::empty())
            .map_err(|error| ExecutorError::Orchestrator(format!("no random bytes: {error}")))?;
    }
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl GaonExecutor {
    /// An executor that creates its objects through `api`, in `image` as `user`, and tells
    /// MicroVMs to post their results to `result_url`, which `results` accepts.
    #[must_use]
    pub fn new(api: Api, image: String, user: JobUser, result_url: &str, results: Results) -> Self {
        Self {
            api,
            image,
            user,
            result_url: result_url.trim_end_matches('/').to_owned(),
            results,
            poll: POLL,
            gone_wait: GONE_WAIT,
        }
    }

    /// The same executor, polling every `poll` and waiting `gone_wait` for a delete.
    #[cfg(test)]
    #[must_use]
    const fn with_timing(mut self, poll: Duration, gone_wait: Duration) -> Self {
        self.poll = poll;
        self.gone_wait = gone_wait;
        self
    }

    /// Deletes every MicroVM and then every Secret that carries the runner's label: what a runner
    /// that ended in the middle of a job left.
    ///
    /// # Errors
    ///
    /// An [`ExecutorError`] where gaon cannot be listed or an object does not leave.
    pub async fn clean(&self) -> Result<(), ExecutorError> {
        self.purge(Kind::MicroVm, None).await?;
        self.purge(Kind::Secret, None).await
    }

    /// Deletes the labelled objects of `kind` (only `only`, where given), until a labelled list
    /// no longer holds them. A delete of an object that is gone answers Forbidden, so the list is
    /// the only evidence that it is.
    async fn purge(&self, kind: Kind, only: Option<&str>) -> Result<(), ExecutorError> {
        let deadline = Instant::now() + self.gone_wait;
        loop {
            let held: Vec<Listed> = self
                .api
                .list(kind)
                .await
                .map_err(|error| orchestrator(&error))?
                .into_iter()
                .filter(|object| only.is_none_or(|name| object.name == name))
                .collect();
            if held.is_empty() {
                return Ok(());
            }
            for object in held.iter().filter(|object| !object.deleting) {
                match self
                    .api
                    .delete(kind, &object.name, &object.resource_version)
                    .await
                {
                    Ok(()) => {}
                    // Gone, or changed since the list: the next list says which.
                    Err(error) if [403, 404, 409].iter().any(|code| error.is_status(*code)) => {}
                    Err(error) => return Err(orchestrator(&error)),
                }
            }
            if Instant::now() >= deadline {
                return Err(ExecutorError::Orchestrator(format!(
                    "{kind:?} objects are still held {} seconds after their delete",
                    self.gone_wait.as_secs()
                )));
            }
            tokio::time::sleep(self.poll).await;
        }
    }

    fn secret_body(&self, name: &str, description: &[u8]) -> Value {
        json!({
            "apiVersion": API_VERSION,
            "kind": "Secret",
            "metadata": { "name": name, "labels": self.api.labels() },
            "data": { DESCRIPTION_KEY: STANDARD.encode(description) },
        })
    }

    fn microvm_body(&self, name: &str) -> Value {
        json!({
            "apiVersion": API_VERSION,
            "kind": "MicroVM",
            "metadata": { "name": name, "labels": self.api.labels() },
            "spec": {
                "image": self.image,
                "user": self.user.as_str(),
                "resources": { "requests": {}, "limits": {} },
                "runStrategy": "Once",
                "command": ["codehub-runner", "run-job", "--job", DESCRIPTION_PATH],
                "files": [{
                    "path": DESCRIPTION_PATH,
                    "mode": 0o400,
                    "valueFrom": { "secretKeyRef": { "name": name, "key": DESCRIPTION_KEY } },
                }],
            },
        })
    }

    /// Creates the job's objects and waits for the MicroVM's end.
    async fn drive(
        &self,
        name: &str,
        description: &[u8],
        stop: &mut Stop,
        expectation: &mut Expectation,
    ) -> Result<Execution, ExecutorError> {
        self.api
            .create(Kind::Secret, name, &self.secret_body(name, description))
            .await
            .map_err(|error| orchestrator(&error))?;
        self.api
            .create(Kind::MicroVm, name, &self.microvm_body(name))
            .await
            .map_err(|error| orchestrator(&error))?;
        loop {
            match self.api.phase(name).await {
                Ok(phase) if phase.is_final() => {
                    return expectation.take().map_or_else(
                        || {
                            Err(ExecutorError::NoResult {
                                phase: phase.phase,
                                reason: phase.reason,
                            })
                        },
                        |posted| Ok(posted.into()),
                    );
                }
                Ok(_) => {}
                Err(error) if error.is_status(403) || error.is_status(404) => {
                    return Err(ExecutorError::Orchestrator(format!(
                        "the MicroVM {name} is gone before it ended: {error}"
                    )));
                }
                Err(error) => eprintln!("job {name}: reading the MicroVM failed: {error}"),
            }
            tokio::select! {
                reason = stop.requested() => {
                    return Ok(Execution { end: End::Stopped(reason), output: String::new() });
                }
                () = tokio::time::sleep(self.poll) => {}
            }
        }
    }

    /// Deletes the MicroVM, and the Secret once the MicroVM has left the labelled list. A failure
    /// leaves the objects for the next start to remove.
    async fn cleanup(&self, name: &str) {
        let result = match self.purge(Kind::MicroVm, Some(name)).await {
            Ok(()) => self.purge(Kind::Secret, Some(name)).await,
            Err(error) => Err(error),
        };
        if let Err(error) = result {
            eprintln!("job {name}: removing its objects failed: {error}");
        }
    }
}

impl Executor for GaonExecutor {
    async fn run(&self, job: &Job, mut stop: Stop) -> Result<Execution, ExecutorError> {
        let id = job.id.value();
        let name = format!("codehub-job-{id}");
        let secret = new_secret()?;
        let url = format!("{}/results/{id}", self.result_url);
        let description = serde_json::to_vec(&Description::new(job, url, secret.clone()))
            .map_err(|error| ExecutorError::Orchestrator(error.to_string()))?;
        let size = description.len() + DESCRIPTION_KEY.len();
        if size > DESCRIPTION_LIMIT {
            return Err(ExecutorError::DescriptionTooLarge {
                size,
                limit: DESCRIPTION_LIMIT,
            });
        }
        let mut expectation = self.results.expect(id, &secret);
        let outcome = self
            .drive(&name, &description, &mut stop, &mut expectation)
            .await;
        self.cleanup(&name).await;
        outcome
    }
}

#[cfg(test)]
mod tests;
