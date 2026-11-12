//! The HTTP adapter of the job source port.

use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::job_source::{
    Address, Download, Heartbeat, Job, JobFile, JobId, JobSource, Report, SourceError, Status,
    StopReason,
};
use crate::token::TokenSource;

/// How long a claim may take. The server holds it, so this exceeds any hold.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(300);

/// How long any other request may take. A heartbeat that waits longer than the server's lease
/// would let the job be handed to another runner while this one still runs it.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct WireJob {
    id: u64,
    organisation: String,
    repository_id: u64,
    changeset: String,
    check: WireCheck,
    heartbeat_interval: u64,
    files: Vec<WireFile>,
}

#[derive(Deserialize)]
struct WireCheck {
    name: String,
    command: String,
}

#[derive(Deserialize)]
struct WireFile {
    path: String,
    address: String,
    executable: bool,
    download: WireGrant,
}

#[derive(Deserialize)]
struct WireGrant {
    url: String,
    headers: Vec<WireHeader>,
}

#[derive(Deserialize)]
struct WireHeader {
    name: String,
    value: String,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum WireHeartbeat {
    Continue,
    Stop { reason: WireReason },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WireReason {
    Cancelled,
    TimedOut,
}

#[derive(Serialize)]
struct WireResult<'a> {
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    output_tail: &'a str,
}

fn address(text: &str) -> Result<Address, SourceError> {
    text.parse()
        .map_err(|error| SourceError::Malformed(format!("{text:?}: {error}")))
}

impl WireJob {
    fn into_job(self) -> Result<Job, SourceError> {
        let files = self
            .files
            .into_iter()
            .map(|file| {
                Ok(JobFile {
                    path: file.path.parse().map_err(|error| {
                        SourceError::Malformed(format!("{:?}: {error}", file.path))
                    })?,
                    address: address(&file.address)?,
                    executable: file.executable,
                    download: Download {
                        url: file.download.url,
                        headers: file
                            .download
                            .headers
                            .into_iter()
                            .map(|header| (header.name, header.value))
                            .collect(),
                    },
                })
            })
            .collect::<Result<_, SourceError>>()?;
        Ok(Job {
            id: JobId::new(self.id),
            organisation: self.organisation,
            repository: self.repository_id,
            changeset: address(&self.changeset)?,
            check: self.check.name,
            command: self.check.command,
            heartbeat_interval: Duration::from_secs(self.heartbeat_interval),
            files,
        })
    }
}

const fn status_name(status: Status) -> &'static str {
    match status {
        Status::Passed => "passed",
        Status::Failed => "failed",
        Status::TimedOut => "timed_out",
        Status::Cancelled => "cancelled",
    }
}

/// Talks to a codehub server's runner API.
pub struct HttpJobSource {
    http: reqwest::Client,
    server: String,
    tokens: TokenSource,
    call_timeout: Duration,
}

impl HttpJobSource {
    /// A source for the server at `server`, authenticating with tokens from `tokens`.
    #[must_use]
    pub fn new(http: reqwest::Client, server: &str, tokens: TokenSource) -> Self {
        Self {
            http,
            server: server.trim_end_matches('/').to_owned(),
            tokens,
            call_timeout: CALL_TIMEOUT,
        }
    }

    /// The same source, with `timeout` as the longest a heartbeat or a result report may take.
    #[must_use]
    pub const fn with_call_timeout(mut self, timeout: Duration) -> Self {
        self.call_timeout = timeout;
        self
    }

    async fn post(
        &self,
        path: &str,
        timeout: Duration,
        body: Option<&impl Serialize>,
    ) -> Result<reqwest::Response, SourceError> {
        let token = self
            .tokens
            .token()
            .await
            .map_err(|error| SourceError::Token(error.to_string()))?;
        let mut request = self
            .http
            .post(format!("{}{path}", self.server))
            .bearer_auth(token)
            .timeout(timeout);
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request
            .send()
            .await
            .map_err(|error| SourceError::Transport(error.to_string()))?;
        match response.status() {
            StatusCode::NOT_FOUND => Err(SourceError::NotHeld),
            status if status.is_success() => Ok(response),
            status => Err(SourceError::Status(status.as_u16())),
        }
    }
}

impl JobSource for HttpJobSource {
    async fn claim(&self) -> Result<Option<Job>, SourceError> {
        let response = self
            .post("/v1/runner/jobs/claim", CLAIM_TIMEOUT, None::<&()>)
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let wire: WireJob = response
            .json()
            .await
            .map_err(|error| SourceError::Malformed(error.to_string()))?;
        wire.into_job().map(Some)
    }

    async fn heartbeat(&self, job: JobId) -> Result<Heartbeat, SourceError> {
        let response = self
            .post(
                &format!("/v1/runner/jobs/{job}/heartbeat"),
                self.call_timeout,
                None::<&()>,
            )
            .await?;
        let wire: WireHeartbeat = response
            .json()
            .await
            .map_err(|error| SourceError::Malformed(error.to_string()))?;
        Ok(match wire {
            WireHeartbeat::Continue => Heartbeat::Continue,
            WireHeartbeat::Stop {
                reason: WireReason::Cancelled,
            } => Heartbeat::Stop(StopReason::Cancelled),
            WireHeartbeat::Stop {
                reason: WireReason::TimedOut,
            } => Heartbeat::Stop(StopReason::TimedOut),
        })
    }

    async fn report(&self, job: JobId, report: &Report) -> Result<(), SourceError> {
        let body = WireResult {
            status: status_name(report.status),
            exit_code: report.exit_code,
            output_tail: &report.output,
        };
        self.post(
            &format!("/v1/runner/jobs/{job}/result"),
            self.call_timeout,
            Some(&body),
        )
        .await
        .map(|_| ())
    }
}

#[cfg(test)]
mod tests;
