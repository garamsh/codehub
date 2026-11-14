//! The HTTP adapter of the job source port.

use std::time::Duration;

use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::job_source::{
    Address, Download, Heartbeat, Job, JobFile, JobId, JobSource, Produced, ProducedFile, Report,
    SourceError, Status, StopReason,
};
use crate::token::TokenSource;

/// How long a claim may take. The server holds it, so this exceeds any hold.
const CLAIM_TIMEOUT: Duration = Duration::from_secs(300);

/// How long the upload of one file may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(600);

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
    id: String,
    folder: String,
    command: String,
    outputs: Vec<String>,
    caches: Vec<String>,
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
    outputs: Vec<WireProduced>,
    caches: Vec<WireProduced>,
}

#[derive(Serialize)]
struct WireProduced {
    path: String,
    address: String,
    executable: bool,
}

impl WireProduced {
    fn of(files: &[ProducedFile]) -> Vec<Self> {
        files
            .iter()
            .map(|file| Self {
                path: file.path.to_string(),
                address: file.address.to_string(),
                executable: file.executable,
            })
            .collect()
    }
}

#[derive(Serialize)]
struct WireUploads {
    files: Vec<WireUpload>,
}

#[derive(Serialize)]
struct WireUpload {
    address: String,
    size: usize,
}

#[derive(Deserialize)]
struct WireUploadGrants {
    grants: Vec<WireUploadGrant>,
}

#[derive(Deserialize)]
struct WireUploadGrant {
    address: String,
    grant: WireGrant,
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
        let paths = |texts: Vec<String>| {
            texts
                .into_iter()
                .map(|text| {
                    text.parse()
                        .map_err(|error| SourceError::Malformed(format!("{text:?}: {error}")))
                })
                .collect::<Result<Vec<_>, SourceError>>()
        };
        Ok(Job {
            id: JobId::new(self.id),
            organisation: self.organisation,
            repository: self.repository_id,
            changeset: address(&self.changeset)?,
            check: self.check.id,
            folder: self.check.folder,
            outputs: paths(self.check.outputs)?,
            caches: paths(self.check.caches)?,
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

    /// Uploads the files of `produced` that the server's object store lacks.
    async fn upload(&self, job: JobId, produced: &Produced) -> Result<(), SourceError> {
        let files: Vec<&ProducedFile> = produced.outputs.iter().chain(&produced.caches).collect();
        if files.is_empty() {
            return Ok(());
        }
        let request = WireUploads {
            files: files
                .iter()
                .map(|file| WireUpload {
                    address: file.address.to_string(),
                    size: file.bytes.len(),
                })
                .collect(),
        };
        let granted: WireUploadGrants = self
            .post(
                &format!("/v1/runner/jobs/{job}/uploads"),
                self.call_timeout,
                Some(&request),
            )
            .await?
            .json()
            .await
            .map_err(|error| SourceError::Malformed(error.to_string()))?;
        for grant in granted.grants {
            let wanted = address(&grant.address)?;
            let Some(file) = files.iter().find(|file| file.address == wanted) else {
                continue;
            };
            let mut put = self
                .http
                .put(&grant.grant.url)
                .timeout(UPLOAD_TIMEOUT)
                .body(file.bytes.clone());
            for header in grant.grant.headers {
                put = put.header(header.name, header.value);
            }
            let response = put
                .send()
                .await
                .map_err(|error| SourceError::Transport(error.without_url().to_string()))?;
            if !response.status().is_success() {
                return Err(SourceError::Status(response.status().as_u16()));
            }
        }
        Ok(())
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
        if let Err(error) = self.upload(job, &report.produced).await {
            eprintln!("job {job}: uploading its files failed: {error}");
        }
        let body = WireResult {
            status: status_name(report.status),
            exit_code: report.exit_code,
            output_tail: &report.output,
            outputs: WireProduced::of(&report.produced.outputs),
            caches: WireProduced::of(&report.produced.caches),
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
