//! The job queue: the jobs of landed changesets, handed to runners one at a time (ADR 0034).
//!
//! Every operation that depends on time takes `now` in seconds since 1970-01-01T00:00:00Z. The
//! queue never reads a clock, and it never moves a job on its own: a lapsed lease or a passed
//! deadline is acted on where [`JobQueue::expire`] is called.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::job::{Heartbeat, Job, JobId, JobResult, NewJob};
use crate::domain::name::OrganisationName;
use crate::domain::repository::RepositoryId;
use crate::domain::runner::{Registration, RegistrationId};

/// Holds the jobs.
pub trait JobQueue {
    /// Queues `jobs` and returns their identifiers in the order given. Identifiers grow with the
    /// order queued.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails; no job is then queued.
    fn queue(
        &self,
        jobs: Vec<NewJob>,
    ) -> impl Future<Output = Result<Vec<JobId>, JobQueueError>> + Send;

    /// Records `job` as failed without running it, with `output` as what it says, and returns it
    /// as recorded.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    fn record_failed(
        &self,
        job: NewJob,
        output: Vec<u8>,
    ) -> impl Future<Output = Result<Job, JobQueueError>> + Send;

    /// Gives the oldest queued job that one of `registrations` matches to that registration for
    /// `lease` seconds from `now`, and returns it as running. Its deadline is its check's timeout
    /// from `now`. `None` where no queued job matches.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> impl Future<Output = Result<Option<Job>, JobQueueError>> + Send;

    /// Extends the lease on `job` to `lease` seconds from `now`, where it runs under one of
    /// `runners`. Where an administrator cancelled the job or it timed out while one of `runners`
    /// held it, answers that the runner is to stop.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `job` is not running, cancelled or timed out under one of
    /// `runners`.
    fn heartbeat(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> impl Future<Output = Result<Heartbeat, JobQueueError>> + Send;

    /// Ends `job` as `result` says, where it runs under one of `runners`, and returns it as it
    /// now stands.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `job` is not running under one of `runners`.
    fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
    ) -> impl Future<Output = Result<Job, JobQueueError>> + Send;

    /// Cancels the queued or running `job` of `organisation` and returns it as it now stands.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::NotFound`] where `organisation` has no such job, and
    /// [`JobQueueError::Finished`] where the job has ended.
    fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> impl Future<Output = Result<Job, JobQueueError>> + Send;

    /// The jobs of `changeset` in `repository`, oldest first.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> impl Future<Output = Result<Vec<Job>, JobQueueError>> + Send;

    /// Acts on time having passed to `now`. A running job past its deadline times out, and a
    /// running job whose lease lapsed before its deadline returns to the queue with no runner.
    /// Returns the jobs that timed out.
    ///
    /// # Errors
    ///
    /// [`JobQueueError::Backend`] where the queue fails.
    fn expire(&self, now: i64) -> impl Future<Output = Result<Vec<Job>, JobQueueError>> + Send;
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
