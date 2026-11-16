//! The HTTP adapter of the job source port.

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::config::ExecutorKind;
use crate::job_source::{
    Address, Change, Download, Heartbeat, Job, JobFile, JobId, JobSource, Produced, ProducedFile,
    Report, RunCache, RunOutcome, RunWork, SourceError, Status, StopReason, Stream,
};
use crate::object_cache::ObjectCounts;
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

/// What a claim answers: a run when it names a draft, a check job otherwise.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireClaim {
    Run(WireRunJob),
    Check(WireJob),
}

#[derive(Deserialize)]
struct WireRunJob {
    id: u64,
    organisation: String,
    repository_id: u64,
    draft: String,
    command: String,
    add: Vec<String>,
    outputs: Vec<String>,
    env: BTreeMap<String, String>,
    heartbeat_interval: u64,
    files: Vec<WireFile>,
    caches: Vec<WireRunCache>,
}

#[derive(Deserialize)]
struct WireRunCache {
    path: String,
    files: Vec<WireFile>,
}

#[derive(Serialize)]
struct WireClaimRequest {
    executor: &'static str,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    objects: Option<WireObjects>,
}

#[derive(Serialize)]
struct WireObjects {
    downloaded: usize,
    from_cache: usize,
}

impl WireObjects {
    fn of(counts: Option<ObjectCounts>) -> Self {
        let counts = counts.unwrap_or_default();
        Self {
            downloaded: counts.downloaded,
            from_cache: counts.cached,
        }
    }
}

#[derive(Serialize)]
struct WireRunReport {
    outcome: &'static str,
    exit_code: Option<i32>,
    stdout: Option<WireStream>,
    stderr: Option<WireStream>,
    objects: WireObjects,
    outputs: Vec<WireProduced>,
    caches: Vec<WireProduced>,
    changes: Vec<WireChange>,
    refused: Vec<String>,
}

#[derive(Serialize)]
struct WireStream {
    address: String,
    cut: bool,
}

impl WireStream {
    fn of(stream: Option<&Stream>) -> Option<Self> {
        stream.map(|stream| Self {
            address: stream.address.to_string(),
            cut: stream.cut,
        })
    }
}

#[derive(Serialize)]
struct WireChange {
    path: String,
    kind: &'static str,
    address: Option<String>,
}

impl WireChange {
    fn of(change: &Change) -> Self {
        match change {
            Change::Written(file) => Self {
                path: file.path.to_string(),
                kind: if file.executable {
                    "executable"
                } else {
                    "regular"
                },
                address: Some(file.address.to_string()),
            },
            Change::Deleted(path) => Self {
                path: path.to_string(),
                kind: "deleted",
                address: None,
            },
        }
    }
}

/// How the run ended, from its status and whether its command ran to an exit code.
const fn outcome_name(status: Status, exit_code: Option<i32>) -> &'static str {
    match (status, exit_code) {
        (Status::TimedOut, _) => "timed_out",
        (Status::Cancelled, _) => "cancelled",
        (_, Some(_)) => "exited",
        (_, None) => "executor_failed",
    }
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

fn wire_files(files: Vec<WireFile>) -> Result<Vec<JobFile>, SourceError> {
    files
        .into_iter()
        .map(|file| {
            Ok(JobFile {
                path: file
                    .path
                    .parse()
                    .map_err(|error| SourceError::Malformed(format!("{:?}: {error}", file.path)))?,
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
        .collect()
}

fn wire_paths(texts: Vec<String>) -> Result<Vec<crate::job_source::RelativePath>, SourceError> {
    texts
        .into_iter()
        .map(|text| {
            text.parse()
                .map_err(|error| SourceError::Malformed(format!("{text:?}: {error}")))
        })
        .collect()
}

impl WireRunJob {
    fn into_job(self) -> Result<Job, SourceError> {
        // A workspace with no base has no changeset; the zero address stands for it.
        let changeset = match self.env.get("CODEHUB_CHANGESET") {
            Some(text) => address(text)?,
            None => Address::new([0; 32]),
        };
        let caches = self
            .caches
            .into_iter()
            .map(|cache| {
                Ok(RunCache {
                    path: cache.path.parse().map_err(|error| {
                        SourceError::Malformed(format!("{:?}: {error}", cache.path))
                    })?,
                    files: wire_files(cache.files)?,
                })
            })
            .collect::<Result<Vec<_>, SourceError>>()?;
        let run = RunWork {
            draft: self.draft,
            add: wire_paths(self.add)?,
            env: self.env.into_iter().collect(),
            caches,
        };
        let cache_paths = run.caches.iter().map(|cache| cache.path.clone()).collect();
        Ok(Job {
            id: JobId::new(self.id),
            organisation: self.organisation,
            repository: self.repository_id,
            changeset,
            check: String::new(),
            folder: String::new(),
            outputs: wire_paths(self.outputs)?,
            caches: cache_paths,
            command: self.command,
            heartbeat_interval: Duration::from_secs(self.heartbeat_interval),
            files: wire_files(self.files)?,
            run: Some(run),
        })
    }
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
            run: None,
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
    executor: Option<ExecutorKind>,
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
            executor: None,
        }
    }

    /// The same source, claiming as a runner that runs jobs on `executor`. Without it a claim
    /// names none, and the server takes it as `host`.
    #[must_use]
    pub const fn with_executor(mut self, executor: ExecutorKind) -> Self {
        self.executor = Some(executor);
        self
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
        if let Some(traceparent) = crate::traceparent::for_path(path) {
            request = request.header("traceparent", traceparent);
        }
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

    /// Uploads the objects among `objects` that the server's object store lacks, each once.
    async fn upload(&self, job: JobId, objects: &[(Address, &[u8])]) -> Result<(), SourceError> {
        let mut seen = HashSet::new();
        let objects: Vec<&(Address, &[u8])> = objects
            .iter()
            .filter(|(address, _)| seen.insert(address.to_string()))
            .collect();
        if objects.is_empty() {
            return Ok(());
        }
        let request = WireUploads {
            files: objects
                .iter()
                .map(|(address, bytes)| WireUpload {
                    address: address.to_string(),
                    size: bytes.len(),
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
            let Some((_, bytes)) = objects.iter().find(|(address, _)| *address == wanted) else {
                continue;
            };
            let mut put = self
                .http
                .put(&grant.grant.url)
                .timeout(UPLOAD_TIMEOUT)
                .body(bytes.to_vec());
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

/// Every object a report names, with its bytes.
fn objects_of<'a>(produced: &'a Produced, run: Option<&'a RunOutcome>) -> Vec<(Address, &'a [u8])> {
    let mut objects: Vec<(Address, &[u8])> = produced
        .outputs
        .iter()
        .chain(&produced.caches)
        .map(|file| (file.address, file.bytes.as_slice()))
        .collect();
    if let Some(run) = run {
        for stream in run.stdout.iter().chain(&run.stderr) {
            objects.push((stream.address, stream.bytes.as_slice()));
        }
        for change in &run.changes {
            if let Change::Written(file) = change {
                objects.push((file.address, file.bytes.as_slice()));
            }
        }
    }
    objects
}

#[async_trait]
impl JobSource for HttpJobSource {
    async fn claim(&self) -> Result<Option<Job>, SourceError> {
        let body = self.executor.map(|executor| WireClaimRequest {
            executor: match executor {
                ExecutorKind::Host => "host",
                ExecutorKind::Gaon => "gaon",
            },
        });
        let response = self
            .post("/v1/runner/jobs/claim", CLAIM_TIMEOUT, body.as_ref())
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let wire: WireClaim = response
            .json()
            .await
            .map_err(|error| SourceError::Malformed(error.to_string()))?;
        match wire {
            WireClaim::Run(run) => run.into_job(),
            WireClaim::Check(check) => check.into_job(),
        }
        .map(Some)
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
        let objects = objects_of(&report.produced, report.run.as_ref());
        let uploaded = self.upload(job, &objects).await;
        if let Err(error) = &uploaded {
            eprintln!("job {job}: uploading its files failed: {error}");
        }
        // A run that could not store what it names reports no object, so the report never
        // names one the store may lack.
        let failed;
        let report = if uploaded.is_err() && report.run.is_some() {
            failed = Report {
                status: Status::Failed,
                exit_code: None,
                output: String::new(),
                produced: Produced::default(),
                objects: report.objects,
                run: Some(RunOutcome::default()),
            };
            &failed
        } else {
            report
        };
        let path = format!("/v1/runner/jobs/{job}/result");
        if let Some(run) = &report.run {
            let body = WireRunReport {
                outcome: outcome_name(report.status, report.exit_code),
                exit_code: report.exit_code,
                stdout: WireStream::of(run.stdout.as_ref()),
                stderr: WireStream::of(run.stderr.as_ref()),
                objects: WireObjects::of(report.objects),
                outputs: WireProduced::of(&report.produced.outputs),
                caches: WireProduced::of(&report.produced.caches),
                changes: run.changes.iter().map(WireChange::of).collect(),
                refused: run.refused.clone(),
            };
            return self
                .post(&path, self.call_timeout, Some(&body))
                .await
                .map(|_| ());
        }
        let body = WireResult {
            status: status_name(report.status),
            exit_code: report.exit_code,
            output_tail: &report.output,
            outputs: WireProduced::of(&report.produced.outputs),
            caches: WireProduced::of(&report.produced.caches),
            objects: report.objects.map(|counts| WireObjects::of(Some(counts))),
        };
        self.post(&path, self.call_timeout, Some(&body))
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests;
