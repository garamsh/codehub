//! The runs of the job queue: the `runs` table (ADR 0051).

use app::domain::draft::Instance;
use app::domain::file::{File, FileKind};
use app::domain::name::DraftName;
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::domain::run::{NewRun, Run, RunId, RunSettings, RunState};
use app::port::driven::job_queue::JobQueueError;
use sqlx::{FromRow, PgPool};

use crate::conversion;

/// The columns of a run, in the order [`RunRow`] reads them.
macro_rules! columns {
    () => {
        "id, organisation, repository, issuer, subject, draft, instance, version, base, command, \
         prefix, add_paths, outputs, labels, timeout_seconds, cache_paths, cache_trees, \
         file_paths, file_kinds, file_contents, state, lease_expires_at, deadline"
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
}

fn paths(columns: &[Vec<u8>]) -> Result<Vec<app::domain::path::Path>, String> {
    columns.iter().map(|path| conversion::path(path)).collect()
}

impl RunRow {
    fn run(self) -> Result<Run, String> {
        let state = match (self.state.as_str(), self.lease_expires_at, self.deadline) {
            ("queued", _, _) => RunState::Queued,
            ("running", Some(lease_expires_at), Some(deadline)) => RunState::Running {
                lease_expires_at,
                deadline,
            },
            ("cancelled", _, _) => RunState::Cancelled,
            (other, _, _) => return Err(format!("a stored run has the state {other:?}")),
        };
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
                settings: RunSettings {
                    caches: paths(&self.cache_paths)?.into_iter().collect(),
                    labels: conversion::labels(&self.labels)?,
                    timeout,
                },
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
pub(crate) async fn queue(pool: &PgPool, new: NewRun) -> Result<Run, JobQueueError> {
    let files: Vec<_> = new.files.iter().collect();
    let caches: Vec<_> = new.caches.iter().collect();
    let row = sqlx::query_as::<_, RunRow>(concat!(
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
    .bind(conversion::labels_column(&new.settings.labels))
    .bind(number(new.settings.timeout.seconds(), "timeout")?)
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
pub(crate) async fn find(pool: &PgPool, id: RunId) -> Result<Run, JobQueueError> {
    let row =
        sqlx::query_as::<_, RunRow>(concat!("select ", columns!(), " from runs where id = $1"))
            .bind(number(id.value(), "run")?)
            .fetch_optional(pool)
            .await
            .map_err(backend)?
            .ok_or_else(|| JobQueueError::NotFound(app::domain::job::JobId::new(id.value())))?;
    run(row)
}

/// Ends the run `id` as cancelled.
pub(crate) async fn cancel(pool: &PgPool, id: RunId) -> Result<Run, JobQueueError> {
    let row = sqlx::query_as::<_, RunRow>(concat!(
        "update runs set state = 'cancelled', updated_at = now() \
         where id = $1 and state <> 'cancelled' returning ",
        columns!()
    ))
    .bind(number(id.value(), "run")?)
    .fetch_optional(pool)
    .await
    .map_err(backend)?;
    match row {
        Some(row) => run(row),
        None => {
            find(pool, id).await?;
            Err(JobQueueError::Finished(app::domain::job::JobId::new(
                id.value(),
            )))
        }
    }
}

/// The latest ended run of the workspace.
pub(crate) async fn latest(
    pool: &PgPool,
    repository: RepositoryId,
    principal: &Principal,
    draft: &DraftName,
    instance: Instance,
) -> Result<Option<Run>, JobQueueError> {
    sqlx::query_as::<_, RunRow>(concat!(
        "select ",
        columns!(),
        " from runs where repository = $1 and issuer = $2 and subject = $3 \
         and draft = $4 and instance = $5 and state = 'cancelled' order by id desc limit 1"
    ))
    .bind(conversion::repository_column(repository).map_err(backend)?)
    .bind(principal.issuer())
    .bind(principal.subject())
    .bind(draft.as_str())
    .bind(number(instance.get(), "instance")?)
    .fetch_optional(pool)
    .await
    .map_err(backend)?
    .map(run)
    .transpose()
}
