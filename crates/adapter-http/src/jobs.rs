//! The runner's conversation: claiming a job, keeping it alive and reporting its result.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use app::domain::changeset::Change;
use app::domain::file::{File, FileKind};
use app::domain::job::{Heartbeat, JobId, JobResult, Outcome, Produced, ProducedFile, StopReason};
use app::domain::path::{Path, Prefix};
use app::domain::run::{ObjectCounts, RunId, RunOutcome, RunReport, RunStream};
use app::port::driving::jobs::{Claimed, ClaimedFile, ClaimedRun, Upload};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::ApiState;
use crate::extract::{AuthenticatedRunner, Body, Params, Raw, parse};
use crate::problem::Problem;
use crate::source_control::{GrantBody, now};

#[derive(Serialize)]
pub struct CheckBody {
    id: String,
    folder: String,
    command: String,
    timeout: u64,
    outputs: Vec<String>,
    caches: Vec<String>,
}

/// `prefix` as `/`-separated components, empty for the whole repository.
pub(crate) fn text(prefix: &Prefix) -> String {
    let parts: Vec<_> = prefix
        .components()
        .iter()
        .map(|component| String::from_utf8_lossy(component.as_bytes()).into_owned())
        .collect();
    parts.join("/")
}

#[derive(Serialize)]
pub struct JobFileBody {
    path: String,
    address: String,
    size: u64,
    executable: bool,
    download: GrantBody,
}

#[derive(Serialize)]
pub struct JobBody {
    id: u64,
    organisation: String,
    repository_id: u64,
    changeset: String,
    check: CheckBody,
    heartbeat_interval: u64,
    files: Vec<JobFileBody>,
}

impl JobBody {
    fn new(claimed: Claimed, heartbeat_interval: Duration) -> Self {
        let Claimed { job, files } = claimed;
        let outputs = job.output_prefixes().iter().map(text).collect();
        let caches = job.cache_prefixes().iter().map(text).collect();
        Self {
            id: job.id.value(),
            organisation: job.organisation.to_string(),
            repository_id: job.repository.value(),
            changeset: job.changeset.to_string(),
            check: CheckBody {
                id: job.check.id().to_string(),
                folder: text(&job.check.folder),
                command: job.check.command.as_str().to_owned(),
                timeout: job.check.timeout.seconds(),
                outputs,
                caches,
            },
            heartbeat_interval: heartbeat_interval.as_secs(),
            files: files.into_iter().map(JobFileBody::from).collect(),
        }
    }
}

impl From<ClaimedFile> for JobFileBody {
    fn from(file: ClaimedFile) -> Self {
        Self {
            path: file.path.to_string(),
            address: file.address.to_string(),
            size: file.size,
            executable: matches!(file.kind, app::domain::file::FileKind::Executable),
            download: file.download.into(),
        }
    }
}

#[derive(Serialize)]
pub struct RunCacheBody {
    path: String,
    files: Vec<JobFileBody>,
}

#[derive(Serialize)]
pub struct RunJobBody {
    id: u64,
    organisation: String,
    repository_id: u64,
    draft: String,
    command: String,
    timeout: u64,
    add: Vec<String>,
    outputs: Vec<String>,
    env: BTreeMap<&'static str, String>,
    heartbeat_interval: u64,
    files: Vec<JobFileBody>,
    caches: Vec<RunCacheBody>,
}

impl RunJobBody {
    fn new(claimed: ClaimedRun, heartbeat_interval: Duration) -> Self {
        let ClaimedRun { run, files, caches } = claimed;
        let request = run.request;
        let mut env = BTreeMap::new();
        env.insert("CODEHUB_ORGANISATION", request.organisation.to_string());
        env.insert("CODEHUB_REPOSITORY", request.repository.to_string());
        env.insert("CODEHUB_DRAFT", request.draft.to_string());
        if let Some(base) = request.base {
            env.insert("CODEHUB_CHANGESET", base.to_string());
        }
        Self {
            id: run.id.value(),
            organisation: request.organisation.to_string(),
            repository_id: request.repository.value(),
            draft: request.draft.to_string(),
            command: request.command.as_str().to_owned(),
            timeout: request.timeout.seconds(),
            add: request.add.iter().map(ToString::to_string).collect(),
            outputs: request.outputs.iter().map(ToString::to_string).collect(),
            env,
            heartbeat_interval: heartbeat_interval.as_secs(),
            files: files.into_iter().map(JobFileBody::from).collect(),
            caches: caches
                .into_iter()
                .map(|cache| RunCacheBody {
                    path: cache.path.to_string(),
                    files: cache.files.into_iter().map(JobFileBody::from).collect(),
                })
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
pub enum ClaimBody {
    Check(Box<JobBody>),
    Run(Box<RunJobBody>),
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReasonBody {
    Cancelled,
    TimedOut,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum HeartbeatBody {
    Continue,
    Stop { reason: ReasonBody },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusBody {
    Passed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorBody {
    Host,
    Gaon,
}

/// What a runner says of itself when it claims; the body may be absent.
#[derive(Deserialize, Default)]
pub struct ClaimRequest {
    executor: Option<ExecutorBody>,
}

#[derive(Deserialize)]
pub struct ObjectCountsBody {
    downloaded: u64,
    from_cache: u64,
}

#[derive(Deserialize)]
pub struct ResultBody {
    status: StatusBody,
    exit_code: Option<i32>,
    output_tail: String,
    /// The object counts, which a check's result is accepted with and does not keep.
    #[serde(default, rename = "objects")]
    _objects: Option<ObjectCountsBody>,
    /// The files the command left under its outputs; absent where it left none.
    #[serde(default)]
    outputs: Vec<ProducedBody>,
    /// The files the command left under its caches; absent where it left none.
    #[serde(default)]
    caches: Vec<ProducedBody>,
}

#[derive(Deserialize)]
pub struct ProducedBody {
    path: String,
    address: String,
    executable: bool,
}

impl ProducedBody {
    fn file(self) -> Result<ProducedFile, Problem> {
        Ok(ProducedFile {
            path: parse("path", &self.path)?,
            file: File {
                kind: if self.executable {
                    FileKind::Executable
                } else {
                    FileKind::Regular
                },
                content: parse("address", &self.address)?,
            },
        })
    }
}

#[derive(Deserialize)]
pub struct StreamIn {
    address: String,
    cut: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKindIn {
    Regular,
    Executable,
    Deleted,
}

#[derive(Deserialize)]
pub struct ChangeIn {
    path: String,
    kind: ChangeKindIn,
    address: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOutcomeIn {
    Exited,
    TimedOut,
    Cancelled,
    ExecutorFailed,
}

#[derive(Deserialize)]
pub struct RunReportBody {
    outcome: RunOutcomeIn,
    exit_code: Option<i32>,
    stdout: Option<StreamIn>,
    stderr: Option<StreamIn>,
    objects: ObjectCountsBody,
    outputs: Vec<ProducedBody>,
    caches: Vec<ProducedBody>,
    changes: Vec<ChangeIn>,
    refused: Vec<String>,
}

impl StreamIn {
    fn stream(self) -> Result<RunStream, Problem> {
        Ok(RunStream {
            address: parse("address", &self.address)?,
            cut: self.cut,
        })
    }
}

impl ChangeIn {
    fn change(self) -> Result<(Path, Change), Problem> {
        let path = parse("path", &self.path)?;
        let kind = match self.kind {
            ChangeKindIn::Regular => FileKind::Regular,
            ChangeKindIn::Executable => FileKind::Executable,
            ChangeKindIn::Deleted => {
                if self.address.is_some() {
                    return Err(Problem::bad_request(format!(
                        "{path} is deleted and has an address"
                    )));
                }
                return Ok((path, Change::Deletion));
            }
        };
        let Some(address) = self.address else {
            return Err(Problem::bad_request(format!("{path} has no address")));
        };
        Ok((
            path,
            Change::File(File {
                kind,
                content: parse("address", &address)?,
            }),
        ))
    }
}

impl RunReportBody {
    fn report(self) -> Result<RunReport, Problem> {
        let outcome = match self.outcome {
            RunOutcomeIn::Exited => RunOutcome::Exited,
            RunOutcomeIn::TimedOut => RunOutcome::TimedOut,
            RunOutcomeIn::Cancelled => RunOutcome::Cancelled,
            RunOutcomeIn::ExecutorFailed => RunOutcome::ExecutorFailed,
        };
        // Only a command that exited has an exit code.
        if (outcome == RunOutcome::Exited) != self.exit_code.is_some() {
            return Err(Problem::bad_request(
                "exit_code is given where the outcome is exited, and only there",
            ));
        }
        Ok(RunReport {
            outcome,
            exit_code: self.exit_code,
            stdout: self.stdout.map(StreamIn::stream).transpose()?,
            stderr: self.stderr.map(StreamIn::stream).transpose()?,
            objects: ObjectCounts {
                downloaded: self.objects.downloaded,
                from_cache: self.objects.from_cache,
            },
            outputs: self
                .outputs
                .into_iter()
                .map(ProducedBody::file)
                .collect::<Result<_, _>>()?,
            caches: self
                .caches
                .into_iter()
                .map(ProducedBody::file)
                .collect::<Result<_, _>>()?,
            changes: self
                .changes
                .into_iter()
                .map(ChangeIn::change)
                .collect::<Result<_, _>>()?,
            refused: self.refused,
        })
    }
}

/// A runner's report, of a check's job or of a run's: a run's names its `outcome`, a check's its
/// `status`.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum ReportBody {
    Run(RunReportBody),
    Check(ResultBody),
}

#[derive(Deserialize)]
pub struct UploadsBody {
    files: Vec<UploadBody>,
}

#[derive(Deserialize)]
pub struct UploadBody {
    address: String,
    size: u64,
}

#[derive(Serialize)]
pub struct UploadGrantBody {
    address: String,
    grant: GrantBody,
}

#[derive(Serialize)]
pub struct UploadGrantsBody {
    grants: Vec<UploadGrantBody>,
}

pub async fn claim(
    State(ports): State<Arc<ApiState>>,
    AuthenticatedRunner(caller): AuthenticatedRunner,
    Raw(body): Raw,
) -> Result<Response, Problem> {
    let request: ClaimRequest = if body.is_empty() {
        ClaimRequest::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|error| Problem::bad_request(format!("the body is not valid: {error}")))?
    };
    // A runner that states no executor takes it to be the host.
    let takes_runs = !matches!(request.executor, Some(ExecutorBody::Gaon));
    let settings = ports.settings;
    let mut waited = Duration::ZERO;
    loop {
        // A run has a person waiting for it, so it goes before a check's job.
        if takes_runs && let Some(claimed) = ports.jobs.claim_run(&caller, now()).await? {
            let body = RunJobBody::new(claimed, settings.heartbeat_interval);
            return Ok(Json(ClaimBody::Run(Box::new(body))).into_response());
        }
        if let Some(claimed) = ports.jobs.claim(&caller, now()).await? {
            let body = JobBody::new(claimed, settings.heartbeat_interval);
            return Ok(Json(ClaimBody::Check(Box::new(body))).into_response());
        }
        let remaining = settings.hold.saturating_sub(waited);
        if remaining.is_zero() {
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        let step = settings
            .retry_interval
            .max(Duration::from_millis(1))
            .min(remaining);
        (settings.pause)(step).await;
        waited += step;
    }
}

pub async fn heartbeat(
    State(ports): State<Arc<ApiState>>,
    AuthenticatedRunner(caller): AuthenticatedRunner,
    Params(job): Params<u64>,
) -> Result<Json<HeartbeatBody>, Problem> {
    let answer = ports
        .jobs
        .heartbeat(&caller, JobId::new(job), now())
        .await?;
    Ok(Json(match answer {
        Heartbeat::Continue => HeartbeatBody::Continue,
        Heartbeat::Stop(StopReason::Cancelled) => HeartbeatBody::Stop {
            reason: ReasonBody::Cancelled,
        },
        Heartbeat::Stop(StopReason::TimedOut) => HeartbeatBody::Stop {
            reason: ReasonBody::TimedOut,
        },
    }))
}

pub async fn report(
    State(ports): State<Arc<ApiState>>,
    AuthenticatedRunner(caller): AuthenticatedRunner,
    Params(job): Params<u64>,
    Body(result): Body<ReportBody>,
) -> Result<StatusCode, Problem> {
    let result = match result {
        ReportBody::Check(result) => result,
        ReportBody::Run(report) => {
            ports
                .jobs
                .report_run(&caller, RunId::new(job), report.report()?, now())
                .await?;
            return Ok(StatusCode::NO_CONTENT);
        }
    };
    let outcome = match result.status {
        StatusBody::Passed => Outcome::Passed,
        StatusBody::Failed => Outcome::Failed,
        StatusBody::TimedOut => Outcome::TimedOut,
        StatusBody::Cancelled => Outcome::Cancelled,
    };
    let produced = Produced {
        outputs: result
            .outputs
            .into_iter()
            .map(ProducedBody::file)
            .collect::<Result<_, _>>()?,
        caches: result
            .caches
            .into_iter()
            .map(ProducedBody::file)
            .collect::<Result<_, _>>()?,
    };
    let result = JobResult {
        outcome,
        exit_code: result.exit_code,
        output: result.output_tail.into_bytes(),
    };
    ports
        .jobs
        .report(&caller, JobId::new(job), result, produced, now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn uploads(
    State(ports): State<Arc<ApiState>>,
    AuthenticatedRunner(caller): AuthenticatedRunner,
    Params(job): Params<u64>,
    Body(request): Body<UploadsBody>,
) -> Result<Json<UploadGrantsBody>, Problem> {
    let files = request
        .files
        .into_iter()
        .map(|file| {
            Ok(Upload {
                address: parse("address", &file.address)?,
                size: file.size,
            })
        })
        .collect::<Result<Vec<_>, Problem>>()?;
    let grants = ports
        .jobs
        .upload_grants(&caller, JobId::new(job), files, now())
        .await?;
    Ok(Json(UploadGrantsBody {
        grants: grants
            .into_iter()
            .map(|grant| UploadGrantBody {
                address: grant.address.to_string(),
                grant: grant.grant.into(),
            })
            .collect(),
    }))
}
