use app::domain::address::Address;
use app::domain::job::{Heartbeat, Job, JobId, JobResult, JobState, NewJob, StopReason};
use app::domain::name::OrganisationName;
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use sqlx::{FromRow, PgConnection, PgPool};

use crate::conversion;
use crate::database::Database;

/// The columns of a job, in the order [`JobRow`] reads them.
macro_rules! job_columns {
    () => {
        "id, organisation, repository, changeset, check_name, command, check_labels, blocking, \
         timeout_seconds, state, runner, lease_expires_at, deadline, exit_code, output, revert_landed, revert_conflicts, revert_failed"
    };
}

/// The job queue in PostgreSQL; it numbers jobs from 1 in the order they were queued, leaving
/// gaps where a queueing was undone.
///
/// A claim takes the oldest matching queued job with `for update skip locked`, so concurrent
/// claims never take one job. Only [`JobQueue::expire`] moves a job whose lease or deadline
/// passed.
pub struct PostgresJobQueue {
    pool: PgPool,
}

impl PostgresJobQueue {
    /// A job queue over `database`.
    #[must_use]
    pub fn new(database: &Database) -> Self {
        Self {
            pool: database.pool.clone(),
        }
    }
}

fn backend(error: impl ToString) -> JobQueueError {
    JobQueueError::Backend(error.to_string())
}

#[derive(FromRow)]
struct JobRow {
    id: i64,
    organisation: String,
    repository: i64,
    changeset: Vec<u8>,
    check_name: String,
    command: String,
    check_labels: Vec<String>,
    blocking: bool,
    timeout_seconds: i64,
    state: String,
    runner: Option<i64>,
    lease_expires_at: Option<i64>,
    deadline: Option<i64>,
    exit_code: Option<i32>,
    output: Vec<u8>,
    revert_landed: Option<Vec<u8>>,
    revert_conflicts: Option<Vec<Vec<u8>>>,
    revert_failed: Option<String>,
}

impl JobRow {
    fn job(self) -> Result<Job, JobQueueError> {
        Ok(Job {
            id: conversion::job(self.id).map_err(backend)?,
            organisation: self.organisation.parse().map_err(backend)?,
            repository: conversion::repository(self.repository).map_err(backend)?,
            changeset: conversion::address(self.changeset).map_err(backend)?,
            check: conversion::check(
                &self.check_name,
                &self.command,
                &self.check_labels,
                self.blocking,
                self.timeout_seconds,
            )
            .map_err(backend)?,
            state: conversion::state(&self.state, self.lease_expires_at, self.deadline)
                .map_err(backend)?,
            runner: self
                .runner
                .map(conversion::registration)
                .transpose()
                .map_err(backend)?,
            exit_code: self.exit_code,
            output: self.output,
            revert: conversion::revert(conversion::RevertColumns {
                landed: self.revert_landed,
                conflicts: self.revert_conflicts,
                failed: self.revert_failed,
            })
            .map_err(backend)?,
        })
    }
}

fn jobs(rows: Vec<JobRow>) -> Result<Vec<Job>, JobQueueError> {
    rows.into_iter().map(JobRow::job).collect()
}

fn number(id: JobId) -> Result<i64, JobQueueError> {
    conversion::number_column(id.value(), "job").map_err(backend)
}

fn numbers(runners: &[RegistrationId]) -> Result<Vec<i64>, JobQueueError> {
    runners
        .iter()
        .map(|runner| conversion::number_column(runner.value(), "registration").map_err(backend))
        .collect()
}

async fn insert(
    connection: &mut PgConnection,
    job: NewJob,
    state: JobState,
    output: Vec<u8>,
) -> Result<JobRow, JobQueueError> {
    let repository = conversion::repository_column(job.repository).map_err(backend)?;
    let timeout =
        conversion::number_column(job.check.timeout.seconds(), "timeout").map_err(backend)?;
    sqlx::query_as::<_, JobRow>(concat!(
        "insert into jobs (organisation, repository, changeset, check_name, command, \
         check_labels, blocking, timeout_seconds, state, output) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) returning ",
        job_columns!()
    ))
    .bind(job.organisation.as_str())
    .bind(repository)
    .bind(job.changeset.digest().as_slice())
    .bind(job.check.name.as_str())
    .bind(job.check.command.as_str())
    .bind(conversion::labels_column(&job.check.labels))
    .bind(job.check.blocking)
    .bind(timeout)
    .bind(conversion::state_column(state))
    .bind(output)
    .fetch_one(connection)
    .await
    .map_err(backend)
}

/// Times out the running jobs whose deadline is `now` or before, and queues again those whose
/// lease lapsed before their deadline. Returns the jobs that timed out, oldest first. Rows are
/// locked in identifier order, so two expiries cannot wait on each other.
async fn expire_at(connection: &mut PgConnection, now: i64) -> Result<Vec<Job>, JobQueueError> {
    let mut timed_out = sqlx::query_as::<_, JobRow>(concat!(
        "update jobs set state = 'timed_out', lease_expires_at = null, deadline = null, \
         updated_at = now() \
         where id in ( \
             select id from jobs \
             where state = 'running' and least(lease_expires_at, deadline) <= $1 \
             and deadline <= $1 order by id for update) \
         returning ",
        job_columns!()
    ))
    .bind(now)
    .fetch_all(&mut *connection)
    .await
    .map_err(backend)?;
    sqlx::query(
        "update jobs set state = 'queued', runner = null, lease_expires_at = null, \
         deadline = null, updated_at = now() \
         where id in ( \
             select id from jobs \
             where state = 'running' and least(lease_expires_at, deadline) <= $1 \
             order by id for update)",
    )
    .bind(now)
    .execute(connection)
    .await
    .map_err(backend)?;
    timed_out.sort_by_key(|row| row.id);
    jobs(timed_out)
}

impl JobQueue for PostgresJobQueue {
    async fn queue(&self, new: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let mut ids = Vec::with_capacity(new.len());
        for job in new {
            let row = insert(&mut transaction, job, JobState::Queued, Vec::new()).await?;
            ids.push(conversion::job(row.id).map_err(backend)?);
        }
        transaction.commit().await.map_err(backend)?;
        Ok(ids)
    }

    async fn record_failed(&self, job: NewJob, output: Vec<u8>) -> Result<Job, JobQueueError> {
        let mut connection = self.pool.acquire().await.map_err(backend)?;
        insert(&mut connection, job, JobState::Failed, output)
            .await?
            .job()
    }

    async fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Job>, JobQueueError> {
        let ids = registrations
            .iter()
            .map(|registration| {
                conversion::number_column(registration.id.value(), "registration").map_err(backend)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let claimed = sqlx::query_as::<_, JobRow>(concat!(
            "with next as ( \
                 select j.id as job, r.id as registration \
                 from jobs j \
                 join runner_registrations r \
                     on r.id = any($1) \
                     and r.organisation = j.organisation \
                     and (r.repository is null or r.repository = j.repository) \
                     and j.check_labels <@ r.labels \
                 where j.state = 'queued' \
                 order by j.id, r.id \
                 limit 1 \
                 for update of j skip locked) \
             update jobs set state = 'running', runner = next.registration, \
                 lease_expires_at = $2 + $3, deadline = $2 + jobs.timeout_seconds, \
                 updated_at = now() \
             from next where jobs.id = next.job \
             returning ",
            job_columns!()
        ))
        .bind(ids)
        .bind(now)
        .bind(lease)
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?;
        transaction.commit().await.map_err(backend)?;
        claimed.map(JobRow::job).transpose()
    }

    async fn heartbeat(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError> {
        let id = number(job)?;
        let runners = numbers(runners)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let extended = sqlx::query(
            "update jobs set lease_expires_at = $3 + $4, updated_at = now() \
             where id = $1 and state = 'running' and runner = any($2) and deadline > $3",
        )
        .bind(id)
        .bind(&runners)
        .bind(now)
        .bind(lease)
        .execute(&mut *transaction)
        .await
        .map_err(backend)?
        .rows_affected();
        let stopped = if extended == 1 {
            None
        } else {
            sqlx::query_scalar::<_, String>(
                "select case when state = 'running' and deadline <= $3 then 'timed_out' \
                 else state end from jobs where id = $1 and runner = any($2)",
            )
            .bind(id)
            .bind(&runners)
            .bind(now)
            .fetch_optional(&mut *transaction)
            .await
            .map_err(backend)?
        };
        transaction.commit().await.map_err(backend)?;
        match (extended, stopped.as_deref()) {
            (1, _) => Ok(Heartbeat::Continue),
            (_, Some("cancelled")) => Ok(Heartbeat::Stop(StopReason::Cancelled)),
            (_, Some("timed_out")) => Ok(Heartbeat::Stop(StopReason::TimedOut)),
            _ => Err(JobQueueError::NotFound(job)),
        }
    }

    async fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
    ) -> Result<Job, JobQueueError> {
        sqlx::query_as::<_, JobRow>(concat!(
            "update jobs set state = $3, exit_code = $4, output = $5, lease_expires_at = null, \
             deadline = null, updated_at = now() \
             where id = $1 and state = 'running' and runner = any($2) \
             returning ",
            job_columns!()
        ))
        .bind(number(job)?)
        .bind(numbers(runners)?)
        .bind(conversion::state_column(result.outcome.state()))
        .bind(result.exit_code)
        .bind(result.output)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .ok_or(JobQueueError::NotFound(job))?
        .job()
    }

    async fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> Result<Job, JobQueueError> {
        let id = number(job)?;
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let cancelled = sqlx::query_as::<_, JobRow>(concat!(
            "update jobs set state = 'cancelled', lease_expires_at = null, deadline = null, \
             updated_at = now() \
             where id = $1 and organisation = $2 and state in ('queued', 'running') \
             returning ",
            job_columns!()
        ))
        .bind(id)
        .bind(organisation.as_str())
        .fetch_optional(&mut *transaction)
        .await
        .map_err(backend)?;
        let Some(cancelled) = cancelled else {
            let exists = sqlx::query_scalar::<_, bool>(
                "select exists (select 1 from jobs where id = $1 and organisation = $2)",
            )
            .bind(id)
            .bind(organisation.as_str())
            .fetch_one(&mut *transaction)
            .await
            .map_err(backend)?;
            return Err(if exists {
                JobQueueError::Finished(job)
            } else {
                JobQueueError::NotFound(job)
            });
        };
        transaction.commit().await.map_err(backend)?;
        cancelled.job()
    }

    async fn reserve_revert(&self, job: JobId) -> Result<bool, JobQueueError> {
        let id = number(job)?;
        let reserved = sqlx::query(
            "insert into revert_reservations (repository, changeset, job) \
             select repository, changeset, id from jobs where id = $1 on conflict do nothing",
        )
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(backend)?
        .rows_affected();
        if reserved == 1 {
            return Ok(true);
        }
        let exists =
            sqlx::query_scalar::<_, bool>("select exists (select 1 from jobs where id = $1)")
                .bind(id)
                .fetch_one(&self.pool)
                .await
                .map_err(backend)?;
        if exists {
            Ok(false)
        } else {
            Err(JobQueueError::NotFound(job))
        }
    }

    async fn record_revert(&self, job: JobId, revert: Revert) -> Result<Job, JobQueueError> {
        let columns = conversion::revert_columns(&revert);
        sqlx::query_as::<_, JobRow>(concat!(
            "update jobs set revert_landed = $2, revert_conflicts = $3, revert_failed = $4, \
             updated_at = now() where id = $1 returning ",
            job_columns!()
        ))
        .bind(number(job)?)
        .bind(columns.landed)
        .bind(columns.conflicts)
        .bind(columns.failed)
        .fetch_optional(&self.pool)
        .await
        .map_err(backend)?
        .ok_or(JobQueueError::NotFound(job))?
        .job()
    }

    async fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<Job>, JobQueueError> {
        let rows = sqlx::query_as::<_, JobRow>(concat!(
            "select ",
            job_columns!(),
            " from jobs where repository = $1 and changeset = $2 order by id"
        ))
        .bind(conversion::repository_column(repository).map_err(backend)?)
        .bind(changeset.digest().as_slice())
        .fetch_all(&self.pool)
        .await
        .map_err(backend)?;
        jobs(rows)
    }

    async fn expire(&self, now: i64) -> Result<Vec<Job>, JobQueueError> {
        let mut transaction = self.pool.begin().await.map_err(backend)?;
        let timed_out = expire_at(&mut transaction, now).await?;
        transaction.commit().await.map_err(backend)?;
        Ok(timed_out)
    }
}
