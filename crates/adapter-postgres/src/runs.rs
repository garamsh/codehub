//! The runs of the job queue: the `runs` table (ADR 0051).

use std::collections::BTreeMap;

use app::domain::changeset::Change;
use app::domain::draft::Instance;
use app::domain::file::{File, FileKind};
use app::domain::job::{Heartbeat, JobId, StopReason};
use app::domain::name::DraftName;
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::domain::run::{
    NewRun, NotWritten, ObjectCounts, Run, RunId, RunOutcome, RunResult, RunState, RunStream,
    WriteBack,
};
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::job_queue::JobQueueError;
use sqlx::{FromRow, PgPool};

use crate::conversion;
use crate::retry;
use crate::statements::Statements;

/// The columns of a run, in the order [`RunRow`] reads them.
macro_rules! columns {
    () => {
        "id, organisation, repository, issuer, subject, draft, instance, version, base, command, \
         prefix, add_paths, outputs, labels, timeout_seconds, cache_paths, cache_trees, \
         file_paths, file_kinds, file_contents, state, lease_expires_at, deadline, runner, \
         result_outcome, exit_code, stdout_address, stdout_cut, stderr_address, stderr_cut, \
         objects_downloaded, objects_from_cache, output_tree, change_paths, change_kinds, \
         change_contents, refused, write_reason, write_paths"
    };
}

fn backend(error: impl ToString) -> JobQueueError {
    JobQueueError::Backend(error.to_string())
}

fn number(value: u64, subject: &str) -> Result<i64, JobQueueError> {
    conversion::number_column(value, subject).map_err(backend)
}

fn unsigned(value: i64, subject: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("a stored run {subject} {value} is negative"))
}

const fn kind_column(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Regular => "regular",
        FileKind::Executable => "executable",
    }
}

fn kind(text: &str) -> Result<FileKind, String> {
    match text {
        "regular" => Ok(FileKind::Regular),
        "executable" => Ok(FileKind::Executable),
        other => Err(format!("a stored run file has the unknown kind {other:?}")),
    }
}

/// The column text of a state, matched on every state so that none can be left out of the
/// queries that name states.
const fn state_column(state: &RunState) -> &'static str {
    match state {
        RunState::Queued => "queued",
        RunState::Running { .. } => "running",
        RunState::Reporting { .. } => "reporting",
        RunState::Cancelled => "cancelled",
        RunState::TimedOut => "timed_out",
        RunState::Finished => "finished",
    }
}

/// The column text of each state that has ended, as [`RunState::ended`] says.
fn ended_columns() -> Vec<&'static str> {
    [
        RunState::Queued,
        RunState::Running {
            lease_expires_at: 0,
            deadline: 0,
        },
        RunState::Reporting { deadline: 0 },
        RunState::Cancelled,
        RunState::TimedOut,
        RunState::Finished,
    ]
    .iter()
    .filter(|state| state.ended())
    .map(state_column)
    .collect()
}

#[derive(FromRow)]
struct RunRow {
    id: i64,
    organisation: String,
    repository: i64,
    issuer: String,
    subject: String,
    draft: String,
    instance: i64,
    version: i64,
    base: Option<Vec<u8>>,
    command: String,
    prefix: Vec<u8>,
    add_paths: Vec<Vec<u8>>,
    outputs: Vec<Vec<u8>>,
    labels: Vec<String>,
    timeout_seconds: i64,
    cache_paths: Vec<Vec<u8>>,
    cache_trees: Vec<Vec<u8>>,
    file_paths: Vec<Vec<u8>>,
    file_kinds: Vec<String>,
    file_contents: Vec<Vec<u8>>,
    state: String,
    lease_expires_at: Option<i64>,
    deadline: Option<i64>,
    runner: Option<i64>,
    result_outcome: Option<String>,
    exit_code: Option<i32>,
    stdout_address: Option<Vec<u8>>,
    stdout_cut: Option<bool>,
    stderr_address: Option<Vec<u8>>,
    stderr_cut: Option<bool>,
    objects_downloaded: Option<i64>,
    objects_from_cache: Option<i64>,
    output_tree: Option<Vec<u8>>,
    change_paths: Option<Vec<Vec<u8>>>,
    change_kinds: Option<Vec<String>>,
    change_contents: Option<Vec<Vec<u8>>>,
    refused: Option<Vec<String>>,
    write_reason: Option<String>,
    write_paths: Option<Vec<Vec<u8>>>,
}

const fn outcome_column(outcome: RunOutcome) -> &'static str {
    match outcome {
        RunOutcome::Exited => "exited",
        RunOutcome::TimedOut => "timed_out",
        RunOutcome::Cancelled => "cancelled",
        RunOutcome::ExecutorFailed => "executor_failed",
    }
}

fn outcome(text: &str) -> Result<RunOutcome, String> {
    match text {
        "exited" => Ok(RunOutcome::Exited),
        "timed_out" => Ok(RunOutcome::TimedOut),
        "cancelled" => Ok(RunOutcome::Cancelled),
        "executor_failed" => Ok(RunOutcome::ExecutorFailed),
        other => Err(format!("a stored run has the outcome {other:?}")),
    }
}

const fn reason_column(reason: NotWritten) -> &'static str {
    match reason {
        NotWritten::InstanceClosed => "instance_closed",
        NotWritten::WorkspaceMoved => "workspace_moved",
        NotWritten::WriteDenied => "write_denied",
        NotWritten::TimedOut => "timed_out",
        NotWritten::Cancelled => "cancelled",
        NotWritten::ExecutorFailed => "executor_failed",
    }
}

fn reason(text: &str) -> Result<NotWritten, String> {
    match text {
        "instance_closed" => Ok(NotWritten::InstanceClosed),
        "workspace_moved" => Ok(NotWritten::WorkspaceMoved),
        "write_denied" => Ok(NotWritten::WriteDenied),
        "timed_out" => Ok(NotWritten::TimedOut),
        "cancelled" => Ok(NotWritten::Cancelled),
        "executor_failed" => Ok(NotWritten::ExecutorFailed),
        other => Err(format!("a stored run has the write-back reason {other:?}")),
    }
}

/// The stream a run stored, where it stored one.
fn stream(address: Option<Vec<u8>>, cut: Option<bool>) -> Result<Option<RunStream>, String> {
    match (address, cut) {
        (Some(address), Some(cut)) => Ok(Some(RunStream {
            address: conversion::address(address)?,
            cut,
        })),
        (None, None) => Ok(None),
        _ => Err("a stored run has half a stream".to_owned()),
    }
}

/// The result a finished run stored.
fn result(row: &mut RunRow, outcome_text: &str) -> Result<RunResult, String> {
    let missing = |column: &str| format!("a finished run has no {column}");
    let kinds = row
        .change_kinds
        .take()
        .ok_or_else(|| missing("change kinds"))?;
    let contents = row
        .change_contents
        .take()
        .ok_or_else(|| missing("change contents"))?;
    let change_paths = paths(
        &row.change_paths
            .take()
            .ok_or_else(|| missing("change paths"))?,
    )?;
    let changes = change_paths
        .into_iter()
        .zip(kinds)
        .zip(contents)
        .map(|((path, kind_text), content)| {
            let change = if kind_text == "deleted" {
                Change::Deletion
            } else {
                Change::File(File {
                    kind: kind(&kind_text)?,
                    content: conversion::address(content)?,
                })
            };
            Ok((path, change))
        })
        .collect::<Result<BTreeMap<_, _>, String>>()?;
    let write_paths = paths(
        &row.write_paths
            .take()
            .ok_or_else(|| missing("write paths"))?,
    )?;
    let write_back = match row.write_reason.take() {
        None => WriteBack::Written { paths: write_paths },
        Some(text) => WriteBack::NotWritten {
            reason: reason(&text)?,
            paths: write_paths,
        },
    };
    let downloaded = row
        .objects_downloaded
        .ok_or_else(|| missing("object count"))?;
    let from_cache = row
        .objects_from_cache
        .ok_or_else(|| missing("object count"))?;
    Ok(RunResult {
        outcome: outcome(outcome_text)?,
        exit_code: row.exit_code,
        stdout: stream(row.stdout_address.take(), row.stdout_cut)?,
        stderr: stream(row.stderr_address.take(), row.stderr_cut)?,
        objects: ObjectCounts {
            downloaded: unsigned(downloaded, "object count")?,
            from_cache: unsigned(from_cache, "object count")?,
        },
        output_tree: row
            .output_tree
            .take()
            .map(conversion::address)
            .transpose()?,
        changes,
        refused: row.refused.take().ok_or_else(|| missing("refused paths"))?,
        write_back,
    })
}

fn paths(columns: &[Vec<u8>]) -> Result<Vec<app::domain::path::Path>, String> {
    columns.iter().map(|path| conversion::path(path)).collect()
}

impl RunRow {
    fn run(mut self) -> Result<Run, String> {
        let state = match (self.state.as_str(), self.lease_expires_at, self.deadline) {
            ("queued", _, _) => RunState::Queued,
            ("running", Some(lease_expires_at), Some(deadline)) => RunState::Running {
                lease_expires_at,
                deadline,
            },
            ("reporting", _, Some(deadline)) => RunState::Reporting { deadline },
            ("cancelled", _, _) => RunState::Cancelled,
            ("timed_out", _, _) => RunState::TimedOut,
            ("finished", _, _) => RunState::Finished,
            (other, _, _) => return Err(format!("a stored run has the state {other:?}")),
        };
        let finished = match self.result_outcome.take() {
            Some(text) => Some(result(&mut self, &text)?),
            None => None,
        };
        let runner = self.runner.map(conversion::registration).transpose()?;
        let kinds = self.file_kinds.iter().map(|text| kind(text));
        let files = paths(&self.file_paths)?
            .into_iter()
            .zip(kinds)
            .zip(self.file_contents)
            .map(|((path, kind), content)| {
                Ok((
                    path,
                    File {
                        kind: kind?,
                        content: conversion::address(content)?,
                    },
                ))
            })
            .collect::<Result<_, String>>()?;
        let caches = paths(&self.cache_paths)?
            .into_iter()
            .zip(self.cache_trees)
            .map(|(path, tree)| Ok((path, conversion::address(tree)?)))
            .collect::<Result<_, String>>()?;
        let timeout = app::domain::check::Timeout::new(unsigned(self.timeout_seconds, "timeout")?)
            .map_err(|error| format!("a stored run has a bad timeout: {error}"))?;
        Ok(Run {
            id: RunId::new(unsigned(self.id, "identifier")?),
            state,
            runner,
            result: finished,
            request: NewRun {
                organisation: self
                    .organisation
                    .parse()
                    .map_err(|error| format!("a stored run has a bad organisation: {error}"))?,
                repository: conversion::repository(self.repository)?,
                principal: Principal::new(&self.issuer, &self.subject)
                    .map_err(|error| format!("a stored run has a bad principal: {error}"))?,
                draft: self
                    .draft
                    .parse()
                    .map_err(|error| format!("a stored run has a bad draft name: {error}"))?,
                instance: Instance::new(unsigned(self.instance, "instance")?),
                version: unsigned(self.version, "version")?,
                base: self.base.map(conversion::address).transpose()?,
                command: app::domain::check::Command::new(&self.command)
                    .map_err(|error| format!("a stored run has a bad command: {error}"))?,
                prefix: conversion::prefix(&self.prefix)?,
                add: paths(&self.add_paths)?.into_iter().collect(),
                outputs: paths(&self.outputs)?.into_iter().collect(),
                labels: conversion::labels(&self.labels)?,
                timeout,
                files,
                caches,
            },
        })
    }
}

fn run(row: RunRow) -> Result<Run, JobQueueError> {
    row.run().map_err(backend)
}

/// Records `new` as queued.
pub(crate) async fn queue(
    pool: &PgPool,
    statements: Statements,
    new: NewRun,
) -> Result<Run, JobQueueError> {
    let files: Vec<_> = new.files.iter().collect();
    let caches: Vec<_> = new.caches.iter().collect();
    let row = statements.query_as::<RunRow>(concat!(
        "insert into runs (organisation, repository, issuer, subject, draft, instance, version, \
         base, command, prefix, add_paths, outputs, labels, timeout_seconds, cache_paths, \
         cache_trees, file_paths, file_kinds, file_contents, state) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, \
         $19, 'queued') returning ",
        columns!()
    ))
    .bind(new.organisation.as_str())
    .bind(conversion::repository_column(new.repository).map_err(backend)?)
    .bind(new.principal.issuer())
    .bind(new.principal.subject())
    .bind(new.draft.as_str())
    .bind(number(new.instance.get(), "instance")?)
    .bind(number(new.version, "version")?)
    .bind(new.base.as_ref().map(|base| base.digest().to_vec()))
    .bind(new.command.as_str())
    .bind(conversion::prefix_column(&new.prefix))
    .bind(conversion::path_columns(
        &new.add.iter().cloned().collect::<Vec<_>>(),
    ))
    .bind(conversion::path_columns(
        &new.outputs.iter().cloned().collect::<Vec<_>>(),
    ))
    .bind(conversion::labels_column(&new.labels))
    .bind(number(new.timeout.seconds(), "timeout")?)
    .bind(
        caches
            .iter()
            .map(|(path, _)| conversion::path_column(path))
            .collect::<Vec<_>>(),
    )
    .bind(
        caches
            .iter()
            .map(|(_, tree)| tree.digest().to_vec())
            .collect::<Vec<_>>(),
    )
    .bind(
        files
            .iter()
            .map(|(path, _)| conversion::path_column(path))
            .collect::<Vec<_>>(),
    )
    .bind(
        files
            .iter()
            .map(|(_, file)| kind_column(file.kind).to_owned())
            .collect::<Vec<_>>(),
    )
    .bind(
        files
            .iter()
            .map(|(_, file)| file.content.digest().to_vec())
            .collect::<Vec<_>>(),
    )
    .fetch_one(pool)
    .await
    .map_err(backend)?;
    run(row)
}

/// The run `id`.
pub(crate) async fn find(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
) -> Result<Run, JobQueueError> {
    let row = statements
        .query_as::<RunRow>(concat!("select ", columns!(), " from runs where id = $1"))
        .bind(number(id.value(), "run")?)
        .fetch_optional(pool)
        .await
        .map_err(backend)?
        .ok_or_else(|| JobQueueError::NotFound(app::domain::job::JobId::new(id.value())))?;
    run(row)
}

/// Ends the run `id` as cancelled.
pub(crate) async fn cancel(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
) -> Result<Run, JobQueueError> {
    let row = statements
        .query_as::<RunRow>(concat!(
            "update runs set state = 'cancelled', lease_expires_at = null, deadline = null, \
         updated_at = now() \
         where id = $1 and state in ('queued', 'running') returning ",
            columns!()
        ))
        .bind(number(id.value(), "run")?)
        .fetch_optional(pool)
        .await
        .map_err(backend)?;
    match row {
        Some(row) => run(row),
        None => {
            find(pool, statements, id).await?;
            Err(JobQueueError::Finished(app::domain::job::JobId::new(
                id.value(),
            )))
        }
    }
}

/// The latest ended run of the workspace.
pub(crate) async fn latest(
    pool: &PgPool,
    statements: Statements,
    repository: RepositoryId,
    principal: &Principal,
    draft: &DraftName,
    instance: Instance,
) -> Result<Option<Run>, JobQueueError> {
    statements
        .query_as::<RunRow>(concat!(
            "select ",
            columns!(),
            " from runs where repository = $1 and issuer = $2 and subject = $3 \
         and draft = $4 and instance = $5 and state = any($6) order by id desc limit 1"
        ))
        .bind(conversion::repository_column(repository).map_err(backend)?)
        .bind(principal.issuer())
        .bind(principal.subject())
        .bind(draft.as_str())
        .bind(number(instance.get(), "instance")?)
        .bind(ended_columns())
        .fetch_optional(pool)
        .await
        .map_err(backend)?
        .map(run)
        .transpose()
}

fn runners(held: &[RegistrationId]) -> Result<Vec<i64>, JobQueueError> {
    held.iter()
        .map(|runner| number(runner.value(), "registration"))
        .collect()
}

fn not_found(id: RunId) -> JobQueueError {
    JobQueueError::NotFound(JobId::new(id.value()))
}

/// Takes the oldest queued run one of `registrations` may take, with `for update skip locked`, so
/// that concurrent claims never take one run.
pub(crate) async fn claim(
    pool: &PgPool,
    statements: Statements,
    registrations: &[Registration],
    now: i64,
    lease: i64,
) -> Result<Option<Run>, JobQueueError> {
    let ids = registrations
        .iter()
        .map(|registration| number(registration.id.value(), "registration"))
        .collect::<Result<Vec<_>, _>>()?;
    // A plain read first: with nothing to take, the claim locks no row.
    let waiting = retry::idempotent(|| async {
        statements
            .query_scalar::<bool>(
                "select exists ( \
                 select 1 from runs u \
                 join runner_registrations r \
                     on r.id = any($1) \
                     and r.organisation = u.organisation \
                     and (r.repository is null or r.repository = u.repository) \
                     and u.labels <@ r.labels \
                 where u.state = 'queued')",
            )
            .bind(&ids)
            .fetch_one(pool)
            .await
    })
    .await
    .map_err(backend)?;
    if !waiting {
        return Ok(None);
    }
    statements
        .query_as::<RunRow>(concat!(
            "with next as ( \
             select u.id as run, r.id as registration \
             from runs u \
             join runner_registrations r \
                 on r.id = any($1) \
                 and r.organisation = u.organisation \
                 and (r.repository is null or r.repository = u.repository) \
                 and u.labels <@ r.labels \
             where u.state = 'queued' \
             order by u.id, r.id \
             limit 1 \
             for update of u skip locked) \
         update runs set state = 'running', runner = next.registration, \
             lease_expires_at = $2 + $3, deadline = $2 + runs.timeout_seconds, \
             updated_at = now() \
         from next where runs.id = next.run \
         returning ",
            columns!()
        ))
        .bind(ids)
        .bind(now)
        .bind(lease)
        .fetch_optional(pool)
        .await
        .map_err(backend)?
        .map(run)
        .transpose()
}

/// Extends the lease on the run `id`, or says why the runner is to stop.
pub(crate) async fn heartbeat(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
    held: &[RegistrationId],
    now: i64,
    lease: i64,
) -> Result<Heartbeat, JobQueueError> {
    let number_of = number(id.value(), "run")?;
    let held = runners(held)?;
    let mut transaction = pool.begin().await.map_err(backend)?;
    let extended = statements
        .query(
            "update runs set lease_expires_at = $3 + $4, updated_at = now() \
         where id = $1 and state = 'running' and runner = any($2) and deadline > $3",
        )
        .bind(number_of)
        .bind(&held)
        .bind(now)
        .bind(lease)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?
        .rows_affected();
    let stopped = if extended == 1 {
        None
    } else {
        statements
            .query_scalar::<String>(
                "select case when state = 'running' and deadline <= $3 then 'timed_out' \
             else state end from runs where id = $1 and runner = any($2)",
            )
            .bind(number_of)
            .bind(&held)
            .bind(now)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?
    };
    transaction.commit().await.map_err(backend)?;
    match (extended, stopped.as_deref()) {
        (1, _) => Ok(Heartbeat::Continue),
        (_, Some("reporting")) => Ok(Heartbeat::Continue),
        (_, Some("cancelled")) => Ok(Heartbeat::Stop(StopReason::Cancelled)),
        (_, Some("timed_out")) => Ok(Heartbeat::Stop(StopReason::TimedOut)),
        _ => Err(not_found(id)),
    }
}

/// The run `id`, where one of `held` holds it and may still upload and report.
pub(crate) async fn held(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
    held: &[RegistrationId],
) -> Result<Run, JobQueueError> {
    let row = statements
        .query_as::<RunRow>(concat!(
            "select ",
            columns!(),
            " from runs where id = $1 and runner = any($2) and (state = 'running' \
         or (state in ('cancelled', 'timed_out') and result_outcome is null))"
        ))
        .bind(number(id.value(), "run")?)
        .bind(runners(held)?)
        .fetch_optional(pool)
        .await
        .map_err(backend)?
        .ok_or_else(|| not_found(id))?;
    run(row)
}

/// Moves the run `id` from running to reporting, with a deadline `lease` seconds from `now`.
pub(crate) async fn begin_report(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
    held: &[RegistrationId],
    now: i64,
    lease: i64,
) -> Result<Run, JobQueueError> {
    let row = statements
        .query_as::<RunRow>(concat!(
            "update runs set state = 'reporting', lease_expires_at = null, deadline = $3 + $4, \
         updated_at = now() \
         where id = $1 and state = 'running' and runner = any($2) returning ",
            columns!()
        ))
        .bind(number(id.value(), "run")?)
        .bind(runners(held)?)
        .bind(now)
        .bind(lease)
        .fetch_optional(pool)
        .await
        .map_err(backend)?
        .ok_or_else(|| not_found(id))?;
    run(row)
}

/// Ends the run `id` with `result`, where one of `held` holds it: a reporting run finishes, and a
/// cancelled or timed-out one that has no result keeps its state and takes `result`.
pub(crate) async fn complete(
    pool: &PgPool,
    statements: Statements,
    id: RunId,
    held: &[RegistrationId],
    result: RunResult,
) -> Result<Run, JobQueueError> {
    let (kinds, contents): (Vec<String>, Vec<Vec<u8>>) = result
        .changes
        .values()
        .map(|change| match change {
            Change::Deletion => ("deleted".to_owned(), Vec::new()),
            Change::File(file) => (
                kind_column(file.kind).to_owned(),
                file.content.digest().to_vec(),
            ),
        })
        .unzip();
    let change_paths: Vec<_> = result.changes.keys().cloned().collect();
    let (not_written, written) = match &result.write_back {
        WriteBack::Written { paths } => (None, paths),
        WriteBack::NotWritten { reason, paths } => (Some(reason_column(*reason)), paths),
    };
    let row = statements.query_as::<RunRow>(concat!(
        "update runs set state = case when state = 'reporting' then 'finished' else state end, \
         lease_expires_at = null, deadline = null, result_outcome = $3, exit_code = $4, stdout_address = $5, stdout_cut = $6, \
         stderr_address = $7, stderr_cut = $8, objects_downloaded = $9, \
         objects_from_cache = $10, output_tree = $11, change_paths = $12, change_kinds = $13, \
         change_contents = $14, refused = $15, write_reason = $16, write_paths = $17, \
         updated_at = now() \
         where id = $1 and runner = any($2) and (state = 'reporting' \
         or (state in ('cancelled', 'timed_out') and result_outcome is null)) \
         returning ",
        columns!()
    ))
    .bind(number(id.value(), "run")?)
    .bind(runners(held)?)
    .bind(outcome_column(result.outcome))
    .bind(result.exit_code)
    .bind(result.stdout.map(|stream| stream.address.digest().to_vec()))
    .bind(result.stdout.map(|stream| stream.cut))
    .bind(result.stderr.map(|stream| stream.address.digest().to_vec()))
    .bind(result.stderr.map(|stream| stream.cut))
    .bind(number(result.objects.downloaded, "object count")?)
    .bind(number(result.objects.from_cache, "object count")?)
    .bind(result.output_tree.map(|tree| tree.digest().to_vec()))
    .bind(conversion::path_columns(&change_paths))
    .bind(kinds)
    .bind(contents)
    .bind(&result.refused)
    .bind(not_written)
    .bind(conversion::path_columns(written))
    .fetch_optional(pool)
    .await
    .map_err(backend)?
    .ok_or_else(|| not_found(id))?;
    run(row)
}

/// Times out the running runs whose deadline is `now` or before, queues again those whose lease
/// lapsed before their deadline, and ends the reporting runs whose deadline is `now` or
/// before as failed in the executor, with nothing written. Rows are locked in identifier order.
pub(crate) async fn expire(
    pool: &PgPool,
    statements: Statements,
    now: i64,
) -> Result<(), JobQueueError> {
    let mut transaction = pool.begin().await.map_err(backend)?;
    statements
        .query(
            "update runs set state = 'timed_out', lease_expires_at = null, deadline = null, \
         updated_at = now() \
         where id in ( \
             select id from runs where state = 'running' and deadline <= $1 \
             order by id for update)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
    statements
        .query(
            "update runs set state = 'queued', runner = null, lease_expires_at = null, \
         deadline = null, updated_at = now() \
         where id in ( \
             select id from runs where state = 'running' and lease_expires_at <= $1 \
             order by id for update)",
        )
        .bind(now)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?;
    statements.query(
        "update runs set state = 'finished', deadline = null, result_outcome = 'executor_failed', \
         objects_downloaded = 0, objects_from_cache = 0, change_paths = array[]::bytea[], \
         change_kinds = array[]::text[], change_contents = array[]::bytea[], \
         refused = array[]::text[], write_reason = 'executor_failed', \
         write_paths = array[]::bytea[], updated_at = now() \
         where id in ( \
             select id from runs where state = 'reporting' and deadline <= $1 \
             order by id for update)",
    )
    .bind(now)
    .execute(&mut *transaction)
    .await
    .map_err(backend)?;
    transaction.commit().await.map_err(backend)
}
