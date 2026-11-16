//! The runs of a workspace: starting one, reading it (and waiting for it to end), cancelling it,
//! the latest one that ended, and its output files (ADR 0051).

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use app::domain::changeset::Change;
use app::domain::check::Command;
use app::domain::file::FileKind;
use app::domain::name::DraftName;
use app::domain::path::{Path, Prefix};
use app::domain::run::{
    NotWritten, Run, RunId, RunOutcome, RunResult, RunState, RunStream, WriteBack,
};
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

const fn outcome(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Exited => "exited",
        RunOutcome::TimedOut => "timed_out",
        RunOutcome::Cancelled => "cancelled",
        RunOutcome::ExecutorFailed => "executor_failed",
    }
}

impl From<RunStream> for StreamBody {
    fn from(stream: RunStream) -> Self {
        Self {
            address: stream.address.to_string(),
            cut: stream.cut,
        }
    }
}

impl From<(Path, Change)> for ChangeBody {
    fn from((path, change): (Path, Change)) -> Self {
        let (kind, address) = match change {
            Change::File(file) => (
                match file.kind {
                    FileKind::Regular => "regular",
                    FileKind::Executable => "executable",
                },
                Some(file.content.to_string()),
            ),
            Change::Deletion => ("deleted", None),
        };
        Self {
            path: path.to_string(),
            kind,
            address,
        }
    }
}

impl From<WriteBack> for WriteBackBody {
    fn from(write_back: WriteBack) -> Self {
        let (written, reason, paths) = match write_back {
            WriteBack::Written { paths } => (true, None, paths),
            WriteBack::NotWritten { reason, paths } => (
                false,
                Some(match reason {
                    NotWritten::InstanceClosed => "instance_closed",
                    NotWritten::WorkspaceMoved => "workspace_moved",
                    NotWritten::WriteDenied => "write_denied",
                    NotWritten::TimedOut => "timed_out",
                    NotWritten::Cancelled => "cancelled",
                    NotWritten::ExecutorFailed => "executor_failed",
                }),
                paths,
            ),
        };
        Self {
            outcome: if written { "written" } else { "not_written" },
            reason,
            paths: paths.iter().map(ToString::to_string).collect(),
        }
    }
}

impl From<RunResult> for ResultBody {
    fn from(result: RunResult) -> Self {
        Self {
            outcome: outcome(result.outcome),
            exit_code: result.exit_code,
            stdout: result.stdout.map(Into::into),
            stderr: result.stderr.map(Into::into),
            objects: ObjectsBody {
                downloaded: result.objects.downloaded,
                from_cache: result.objects.from_cache,
            },
            output_tree: result.output_tree.map(|tree| tree.to_string()),
            changes: result.changes.into_iter().map(Into::into).collect(),
            refused: result.refused,
            write_back: result.write_back.into(),
        }
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
        let result = run.ended().map(ResultBody::from);
        let (state, lease_expires_at, deadline) = match run.state {
            RunState::Queued => (StateBody::Queued, None, None),
            RunState::Running {
                lease_expires_at,
                deadline,
            } => (StateBody::Running, Some(lease_expires_at), Some(deadline)),
            RunState::Reporting { deadline } => (StateBody::Running, None, Some(deadline)),
            RunState::Cancelled | RunState::TimedOut | RunState::Finished => {
                (StateBody::Ended, None, None)
            }
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
