//! The job queue: the jobs of landed changesets, handed to runners one at a time (ADR 0034).
//!
//! Every operation that depends on time takes `now` in seconds since 1970-01-01T00:00:00Z. The
//! queue never reads a clock, and it never moves a job on its own: a lapsed lease or a passed
//! deadline is acted on where [`JobQueue::expire`] is called.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::check::CheckKey;
use crate::domain::draft::Instance;
use crate::domain::job::{Heartbeat, Job, JobId, JobResult, NewJob, Resolution};
use crate::domain::name::{DraftName, OrganisationName};
use crate::domain::principal::Principal;
use crate::domain::repository::RepositoryId;
use crate::domain::revert::Revert;
use crate::domain::run::{NewRun, Run, RunId};
use crate::domain::runner::{Registration, RegistrationId};

/// Holds the jobs.
#[async_trait::async_trait]
pub trait JobQueue: Send + Sync {
    /// Records `jobs` as waiting, for [`JobQueue::resolve`] to queue, and returns their identifiers
    /// in the order given. Identifiers grow with the order recorded. A waiting job is not claimed.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails; no job is then queued.
    async fn queue(&self, jobs: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError>;

    /// Settles the waiting `job` as `resolution` says: queued for a runner under its key, passed
    /// from the cache with the output tree of the run that passed, or skipped. Returns whether it
    /// did; `false` where `job` is no longer waiting, which leaves it as it is.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where there is no such job.
    async fn resolve(&self, job: JobId, resolution: Resolution) -> Result<bool, JobQueueError>;

    /// The output tree of a run in `repository` that passed with `key`; `None` where none did.
    /// Only a run that passed answers: a failed, timed-out or cancelled run is never cached, and
    /// neither is a result taken from the cache, which stands for the run it took it from.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn passed_with_key(
        &self,
        repository: RepositoryId,
        key: CheckKey,
    ) -> Result<Option<Address>, JobQueueError>;

    /// Records `job` as failed without running it, with `output` as what it says, and returns it
    /// as recorded.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn record_failed(&self, job: NewJob, output: Vec<u8>) -> Result<Job, JobQueueError>;

    /// Gives the oldest queued job that one of `registrations` matches to that registration for
    /// `lease` seconds from `now`, and returns it as running. Its deadline is its check's timeout
    /// from `now`. `None` where no queued job matches.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Job>, JobQueueError>;

    /// Extends the lease on `job` to `lease` seconds from `now`, where it runs under one of
    /// `runners`. Where an administrator cancelled the job or it timed out while one of `runners`
    /// held it, or where the job is running and its deadline has passed, answers that the runner is
    /// to stop; a passed deadline is answered as timed out whether or not [`JobQueue::expire`] has
    /// acted on it, and extends nothing.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `job` is not running, cancelled or timed out under one of
    /// `runners`.
    async fn heartbeat(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError>;

    /// `job`, where it runs under one of `runners`.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `job` is not running under one of `runners`.
    async fn running(&self, job: JobId, runners: &[RegistrationId]) -> Result<Job, JobQueueError>;

    /// Ends `job` as `result` says, where it runs under one of `runners`, and returns it as it
    /// now stands. `output_tree` is recorded with it.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `job` is not running under one of `runners`.
    async fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
        output_tree: Address,
    ) -> Result<Job, JobQueueError>;

    /// Cancels the waiting, queued or running `job` of `organisation` and returns it as it now stands.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `organisation` has no such job, and
    /// [`JobQueueError::Finished`] where the job has ended.
    async fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> Result<Job, JobQueueError>;

    /// Reserves the revert of the changeset `job` belongs to, for `job`. Of all the jobs of one
    /// changeset, however many ask at once, exactly one is answered `true`; the others are
    /// answered `false`. A reservation is never released, so a revert that failed after it is not
    /// tried again.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where there is no such job.
    async fn reserve_revert(&self, job: JobId) -> Result<bool, JobQueueError>;

    /// Records `revert` with the result of `job` and returns the job as it now stands, which
    /// completes the reservation of the revert. A later record for the same job replaces the
    /// earlier one.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where there is no such job.
    async fn record_revert(&self, job: JobId, revert: Revert) -> Result<Job, JobQueueError>;

    /// The jobs of `changeset` in `repository`, oldest first.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<Job>, JobQueueError>;

    /// Records `run` as queued for a runner and returns it as recorded. Its identifier comes from
    /// the sequence the jobs are numbered by.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails; no run is then queued.
    async fn queue_run(&self, run: NewRun) -> Result<Run, JobQueueError>;

    /// The run `run`.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where there is no such run.
    async fn run(&self, run: RunId) -> Result<Run, JobQueueError>;

    /// Ends the run `run` as cancelled and returns it.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where there is no such run; [`JobQueueError::Finished`] where
    /// it has ended.
    async fn cancel_run(&self, run: RunId) -> Result<Run, JobQueueError>;

    /// The latest run that has ended of `principal`'s opening `instance` of the workspace `draft`
    /// in `repository`; `None` where none has.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn latest_run(
        &self,
        repository: RepositoryId,
        principal: &Principal,
        draft: &DraftName,
        instance: Instance,
    ) -> Result<Option<Run>, JobQueueError>;

    /// Acts on time having passed to `now`. A running job past its deadline times out, and a
    /// running job whose lease lapsed before its deadline returns to the queue with no runner.
    /// Returns the jobs that timed out.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    async fn expire(&self, now: i64) -> Result<Vec<Job>, JobQueueError>;
}

/// Why the job queue refused or did not answer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JobQueueError {
    /// There is no such job for the caller.
    NotFound(JobId),
    /// The job has ended and cannot be cancelled.
    Finished(JobId),
    /// The queue's backend failed; carries a description.
    Backend(String),
}

impl fmt::Display for JobQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "no job {id}"),
            Self::Finished(id) => write!(f, "job {id} has ended"),
            Self::Backend(description) => write!(f, "job queue failed: {description}"),
        }
    }
}

impl std::error::Error for JobQueueError {}
