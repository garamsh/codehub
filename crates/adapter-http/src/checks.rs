//! The checks of a repository: the jobs of a changeset, cancelling a job, and the refs checks move.

use std::sync::Arc;

use app::domain::address::Address;
use app::domain::job::{Job, JobId, JobState};
use app::port::driving::checks::Checks;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::RunnerServices;
use crate::extract::{Authenticated, Params, find, parse};
use crate::problem::Problem;
use crate::source_control::now;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateBody {
    Queued,
    Running,
    Passed,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Serialize)]
pub struct ListedBody {
    id: u64,
    name: String,
    blocking: bool,
    labels: Vec<String>,
    state: StateBody,
    exit_code: Option<i32>,
    output_tail: String,
    lease_expires_at: Option<i64>,
    deadline: Option<i64>,
}

#[derive(Serialize)]
pub struct ChecksBody {
    checks: Vec<ListedBody>,
}

#[derive(Serialize)]
pub struct RefBody {
    name: &'static str,
    changeset: String,
}

impl From<Job> for ListedBody {
    fn from(job: Job) -> Self {
        let (state, lease_expires_at, deadline) = match job.state {
            JobState::Queued => (StateBody::Queued, None, None),
            JobState::Running {
                lease_expires_at,
                deadline,
            } => (StateBody::Running, Some(lease_expires_at), Some(deadline)),
            JobState::Passed => (StateBody::Passed, None, None),
            JobState::Failed => (StateBody::Failed, None, None),
            JobState::TimedOut => (StateBody::TimedOut, None, None),
            JobState::Cancelled => (StateBody::Cancelled, None, None),
        };
        Self {
            id: job.id.value(),
            name: job.check.name.to_string(),
            blocking: job.check.blocking,
            labels: job.check.labels.iter().map(ToString::to_string).collect(),
            state,
            exit_code: job.exit_code,
            output_tail: String::from_utf8_lossy(&job.output).into_owned(),
            lease_expires_at,
            deadline,
        }
    }
}

pub async fn list<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, changeset)): Params<(String, String, String)>,
) -> Result<Json<ChecksBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let changeset: Address = parse("address", &changeset)?;
    let jobs = ports
        .checks()
        .jobs_of(&caller, repository, changeset, now())
        .await?;
    Ok(Json(ChecksBody {
        checks: jobs.into_iter().map(Into::into).collect(),
    }))
}

pub async fn cancel<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, job)): Params<(String, String, u64)>,
) -> Result<StatusCode, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    ports
        .checks()
        .cancel(&caller, repository, JobId::new(job))
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn reference<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, name)): Params<(String, String, String)>,
) -> Result<Json<RefBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let changeset = match name.as_str() {
        "last-green" => ports.checks().last_green(&caller, repository).await?,
        // No driving port yet names the changeset `main` points at, so `main` is not served.
        _ => return Err(Problem::not_found("no such ref")),
    };
    let changeset = changeset.ok_or_else(|| Problem::not_found("the ref is not set"))?;
    Ok(Json(RefBody {
        name: "last-green",
        changeset: changeset.to_string(),
    }))
}
