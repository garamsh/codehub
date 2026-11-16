//! The runs of a workspace: starting one, reading it (and waiting for it to end), cancelling it,
//! the latest one that ended, and its output files (ADR 0051).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use app::domain::check::Command;
use app::domain::name::DraftName;
use app::domain::path::{Path, Prefix};
use app::domain::run::{Run, RunId, RunState};
use app::port::driving::runs::{OutputFile, StartRun};
use axum::Json;
use axum::extract::State;
use serde::{Deserialize, Serialize};

use crate::ApiState;
use crate::extract::{Authenticated, Body, Params, Queried, find, parse, parse_prefix};
use crate::jobs::text;
use crate::problem::Problem;
use crate::source_control::GrantBody;

/// The longest a read of a run is held for it to end, in seconds.
const MAX_WAIT: u64 = 60;

#[derive(Deserialize)]
pub struct StartBody {
    command: String,
    prefix: Option<String>,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    outputs: Vec<String>,
}

impl StartBody {
    fn request(self) -> Result<StartRun, Problem> {
        let paths = |texts: Vec<String>| -> Result<BTreeSet<Path>, Problem> {
            texts.iter().map(|text| parse("path", text)).collect()
        };
        Ok(StartRun {
            command: Command::new(&self.command)
                .map_err(|error| Problem::bad_request(format!("not a command: {error}")))?,
            prefix: self
                .prefix
                .map(|prefix| parse_prefix(&prefix))
                .transpose()?,
            add: paths(self.add)?,
            outputs: paths(self.outputs)?,
        })
    }
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateBody {
    Queued,
    Running,
    Ended,
}

#[derive(Serialize)]
pub struct ObjectsBody {
    downloaded: u64,
    from_cache: u64,
}

#[derive(Serialize)]
pub struct WriteBackBody {
    outcome: &'static str,
    reason: Option<&'static str>,
    paths: Vec<String>,
}

#[derive(Serialize)]
pub struct StreamBody {
    address: String,
    cut: bool,
}

#[derive(Serialize)]
pub struct ChangeBody {
    path: String,
    kind: &'static str,
    address: Option<String>,
}

#[derive(Serialize)]
pub struct ResultBody {
    outcome: &'static str,
    exit_code: Option<i32>,
    stdout: Option<StreamBody>,
    stderr: Option<StreamBody>,
    objects: ObjectsBody,
    output_tree: Option<String>,
    changes: Vec<ChangeBody>,
    refused: Vec<String>,
    write_back: WriteBackBody,
}

/// The result of a run cancelled before a runner reported one: only the outcome is known.
fn cancelled() -> ResultBody {
    ResultBody {
        outcome: "cancelled",
        exit_code: None,
        stdout: None,
        stderr: None,
        objects: ObjectsBody {
            downloaded: 0,
            from_cache: 0,
        },
        output_tree: None,
        changes: Vec::new(),
        refused: Vec::new(),
        write_back: WriteBackBody {
            outcome: "not_written",
            reason: Some("cancelled"),
            paths: Vec::new(),
        },
    }
}

#[derive(Serialize)]
pub struct RunBody {
    id: u64,
    draft: String,
    instance: u64,
    version: u64,
    command: String,
    prefix: String,
    state: StateBody,
    lease_expires_at: Option<i64>,
    deadline: Option<i64>,
    result: Option<ResultBody>,
}

impl From<Run> for RunBody {
    fn from(run: Run) -> Self {
        let (state, lease_expires_at, deadline, result) = match run.state {
            RunState::Queued => (StateBody::Queued, None, None, None),
            RunState::Running {
                lease_expires_at,
                deadline,
            } => (
                StateBody::Running,
                Some(lease_expires_at),
                Some(deadline),
                None,
            ),
            RunState::Cancelled => (StateBody::Ended, None, None, Some(cancelled())),
        };
        let request = run.request;
        Self {
            id: run.id.value(),
            draft: request.draft.to_string(),
            instance: request.instance.get(),
            version: request.version,
            command: request.command.as_str().to_owned(),
            prefix: text(&request.prefix),
            state,
            lease_expires_at,
            deadline,
            result,
        }
    }
}

#[derive(Deserialize)]
pub struct WaitQuery {
    wait: Option<u64>,
}

#[derive(Deserialize)]
pub struct FilesQuery {
    #[serde(default)]
    prefix: String,
}

#[derive(Serialize)]
pub struct OutputFileBody {
    path: String,
    address: String,
    executable: bool,
    download: GrantBody,
}

#[derive(Serialize)]
pub struct FilesBody {
    files: Vec<OutputFileBody>,
}

impl From<OutputFile> for OutputFileBody {
    fn from(output: OutputFile) -> Self {
        Self {
            path: output.path.to_string(),
            address: output.file.content.to_string(),
            executable: matches!(output.file.kind, app::domain::file::FileKind::Executable),
            download: output.download.into(),
        }
    }
}

pub async fn start(
    State(ports): State<Arc<ApiState>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
    Body(body): Body<StartBody>,
) -> Result<Json<RunBody>, Problem> {
    let repository = find(&ports, &caller, &organisation, &repository).await?;
    let draft: DraftName = parse("draft name", &draft)?;
    let run = ports
        .runs
        .start(&caller, repository, &draft, body.request()?)
        .await?;
    Ok(Json(run.into()))
}

pub async fn get(
    State(ports): State<Arc<ApiState>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, id)): Params<(String, String, u64)>,
    Queried(query): Queried<WaitQuery>,
) -> Result<Json<RunBody>, Problem> {
    let repository = find(&ports, &caller, &organisation, &repository).await?;
    let wait = query.wait.unwrap_or(0);
    if wait > MAX_WAIT {
        return Err(Problem::bad_request(format!(
            "wait is at most {MAX_WAIT} seconds"
        )));
    }
    let settings = ports.settings;
    let hold = Duration::from_secs(wait);
    let mut waited = Duration::ZERO;
    loop {
        let run = ports.runs.get(&caller, repository, RunId::new(id)).await?;
        let remaining = hold.saturating_sub(waited);
        if run.state.ended() || remaining.is_zero() {
            return Ok(Json(run.into()));
        }
        let step = settings
            .retry_interval
            .max(Duration::from_millis(1))
            .min(remaining);
        (settings.pause)(step).await;
        waited += step;
    }
}

pub async fn cancel(
    State(ports): State<Arc<ApiState>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, id)): Params<(String, String, u64)>,
) -> Result<Json<RunBody>, Problem> {
    let repository = find(&ports, &caller, &organisation, &repository).await?;
    let run = ports
        .runs
        .cancel(&caller, repository, RunId::new(id))
        .await?;
    Ok(Json(run.into()))
}

pub async fn latest(
    State(ports): State<Arc<ApiState>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, draft)): Params<(String, String, String)>,
) -> Result<Json<RunBody>, Problem> {
    let repository = find(&ports, &caller, &organisation, &repository).await?;
    let draft: DraftName = parse("draft name", &draft)?;
    let run = ports.runs.latest(&caller, repository, &draft).await?;
    Ok(Json(run.into()))
}

pub async fn files(
    State(ports): State<Arc<ApiState>>,
    Authenticated(caller): Authenticated,
    Params((organisation, repository, id)): Params<(String, String, u64)>,
    Queried(query): Queried<FilesQuery>,
) -> Result<Json<FilesBody>, Problem> {
    let repository = find(&ports, &caller, &organisation, &repository).await?;
    let prefix: Prefix = parse_prefix(&query.prefix)?;
    let files = ports
        .runs
        .files(&caller, repository, RunId::new(id), &prefix)
        .await?;
    Ok(Json(FilesBody {
        files: files.into_iter().map(Into::into).collect(),
    }))
}
