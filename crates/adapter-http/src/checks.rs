//! The checks of a repository: the jobs of a changeset, cancelling a job, the refs checks move, and
//! the plan of a draft.

use std::sync::Arc;

use app::domain::address::Address;
use app::domain::job::{Job, JobId, JobState};
use app::domain::name::DraftName;
use app::domain::revert::Revert;
use app::port::driven::changeset_log::Ref;
use app::port::driving::checks::{Checks, Plan, PlanOutcome, RunReason};
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
    Waiting,
    Queued,
    Running,
    Passed,
    Failed,
    TimedOut,
    Cancelled,
    Skipped,
    Cached,
}

#[derive(Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RevertBody {
    Landed { changeset: String },
    Conflicted { paths: Vec<String> },
    Failed { reason: String },
}

impl From<Revert> for RevertBody {
    fn from(revert: Revert) -> Self {
        match revert {
            Revert::Landed(changeset) => Self::Landed {
                changeset: changeset.to_string(),
            },
            Revert::Conflicted(paths) => Self::Conflicted {
                paths: paths.iter().map(ToString::to_string).collect(),
            },
            Revert::Failed { reason } => Self::Failed { reason },
        }
    }
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
    revert: Option<RevertBody>,
}

#[derive(Serialize)]
pub struct ChecksBody {
    checks: Vec<ListedBody>,
}

#[derive(Serialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum ReasonBody {
    InputsChanged,
    #[serde(rename = "needs_run")]
    NeedRuns {
        check: String,
    },
    NoPassedRun,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlanOutcomeBody {
    Run,
    Cached,
}

#[derive(Serialize)]
pub struct PlannedBody {
    name: String,
    blocking: bool,
    inputs: Vec<String>,
    outcome: PlanOutcomeBody,
    reasons: Vec<ReasonBody>,
}

#[derive(Serialize)]
pub struct PlanBody {
    checks: Vec<PlannedBody>,
}

impl From<Plan> for PlanBody {
    fn from(plan: Plan) -> Self {
        let checks = plan
            .checks
            .into_iter()
            .map(|planned| {
                let (outcome, reasons) = match planned.outcome {
                    PlanOutcome::Cached => (PlanOutcomeBody::Cached, Vec::new()),
                    PlanOutcome::Run(reasons) => (
                        PlanOutcomeBody::Run,
                        reasons
                            .into_iter()
                            .map(|reason| match reason {
                                RunReason::InputsChanged => ReasonBody::InputsChanged,
                                RunReason::NeedRuns(check) => ReasonBody::NeedRuns {
                                    check: check.to_string(),
                                },
                                RunReason::NoPassedRun => ReasonBody::NoPassedRun,
                            })
                            .collect(),
                    ),
                };
                PlannedBody {
                    name: planned.check.to_string(),
                    blocking: planned.blocking,
                    inputs: planned.inputs.iter().map(ToString::to_string).collect(),
                    outcome,
                    reasons,
                }
            })
            .collect();
        Self { checks }
    }
}

#[derive(Serialize)]
pub struct RefBody {
    name: &'static str,
    changeset: String,
}

impl From<Job> for ListedBody {
    fn from(job: Job) -> Self {
        let (state, lease_expires_at, deadline) = match job.state {
            JobState::Waiting => (StateBody::Waiting, None, None),
            JobState::Queued => (StateBody::Queued, None, None),
            JobState::Running {
                lease_expires_at,
                deadline,
            } => (StateBody::Running, Some(lease_expires_at), Some(deadline)),
            JobState::Passed => (StateBody::Passed, None, None),
            JobState::Failed => (StateBody::Failed, None, None),
            JobState::TimedOut => (StateBody::TimedOut, None, None),
            JobState::Cancelled => (StateBody::Cancelled, None, None),
            JobState::Skipped => (StateBody::Skipped, None, None),
            JobState::Cached => (StateBody::Cached, None, None),
        };
        Self {
            id: job.id.value(),
            name: job.check.id().to_string(),
            blocking: job.check.blocking,
            labels: job.check.labels.iter().map(ToString::to_string).collect(),
            state,
            exit_code: job.exit_code,
            output_tail: String::from_utf8_lossy(&job.output).into_owned(),
            lease_expires_at,
            deadline,
            revert: job.revert.map(Into::into),
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
    let (name, reference) = match name.as_str() {
        "main" => ("main", Ref::Main),
        "last-green" => ("last-green", Ref::LastGreen),
        _ => return Err(Problem::not_found("no such ref")),
    };
    let changeset = ports
        .checks()
        .reference(&caller, repository, reference)
        .await?;
    let changeset = changeset.ok_or_else(|| Problem::not_found("the ref is not set"))?;
    Ok(Json(RefBody {
        name,
        changeset: changeset.to_string(),
    }))
}

pub async fn plan<P: RunnerServices>(
    State(ports): State<Arc<P>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
) -> Result<Json<PlanBody>, Problem> {
    let repository = find(&*ports, &caller, &organisation, &repository).await?;
    let draft: DraftName = parse("draft name", &draft)?;
    let plan = ports.checks().plan(&caller, repository, &draft).await?;
    Ok(Json(plan.into()))
}
