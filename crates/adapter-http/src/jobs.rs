//! The runner's conversation: claiming a job, keeping it alive and reporting its result.

use std::sync::Arc;
use std::time::Duration;

use app::domain::job::{Heartbeat, JobId, JobResult, Outcome, StopReason};
use app::port::driving::jobs::{Claimed, ClaimedFile, Jobs};
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::RunnerServices;
use crate::extract::{Authenticated, Body, Params};
use crate::problem::Problem;
use crate::source_control::{GrantBody, now};

#[derive(Serialize)]
pub struct CheckBody {
    name: String,
    command: String,
    timeout: u64,
}

#[derive(Serialize)]
pub struct JobFileBody {
    path: String,
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
        Self {
            id: job.id.value(),
            organisation: job.organisation.to_string(),
            repository_id: job.repository.value(),
            changeset: job.changeset.to_string(),
            check: CheckBody {
                name: job.check.name.to_string(),
                command: job.check.command.as_str().to_owned(),
                timeout: job.check.timeout.seconds(),
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
    let result = JobResult {
        outcome,
        exit_code: result.exit_code,
        output: result.output_tail.into_bytes(),
    };
    ports
        .jobs()
        .report(&caller, JobId::new(job), result, now())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
