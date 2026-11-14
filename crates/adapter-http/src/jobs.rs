//! The runner's conversation: claiming a job, keeping it alive and reporting its result.

use std::sync::Arc;
use std::time::Duration;

use app::domain::file::{File, FileKind};
use app::domain::job::{Heartbeat, JobId, JobResult, Outcome, Produced, ProducedFile, StopReason};
use app::domain::path::Prefix;
use app::port::driving::jobs::{Claimed, ClaimedFile, Jobs, Upload};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::RunnerServices;
use crate::extract::{Authenticated, Body, Params, parse};
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
fn text(prefix: &Prefix) -> String {
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
pub struct ResultBody {
    status: StatusBody,
    exit_code: Option<i32>,
    output_tail: String,
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

pub async fn claim<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
) -> Result<Response, Problem> {
    let settings = ports.settings();
    let mut waited = Duration::ZERO;
    loop {
        if let Some(claimed) = ports.jobs().claim(&caller, now()).await? {
            return Ok(Json(JobBody::new(claimed, settings.heartbeat_interval)).into_response());
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

pub async fn heartbeat<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(job): Params<u64>,
) -> Result<Json<HeartbeatBody>, Problem> {
    let answer = ports
        .jobs()
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

pub async fn report<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params(job): Params<u64>,
    Body(result): Body<ResultBody>,
) -> Result<StatusCode, Problem> {
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
        .jobs()
        .report(&caller, JobId::new(job), result, produced, now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn uploads<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
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
        .jobs()
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
