//! Checks, as a reader of a repository sees them: the jobs of a changeset, `last-green`, and
//! cancelling a job.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::job::{Job, JobId};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driving::access::AccessError;

/// What the outside asks of a repository's checks.
pub trait Checks {
    /// The jobs of `changeset` in `repository`, oldest first, as they stand at `now` in seconds
    /// since 1970-01-01T00:00:00Z.
    ///
    /// # Errors
    ///
    /// [`ChecksError::Access`] where `repository` does not exist or `caller` holds no read on it.
    fn jobs_of(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        now: i64,
    ) -> impl Future<Output = Result<Vec<Job>, ChecksError>> + Send;

    /// The changeset `last-green` points at; `None` before it is first set.
    ///
    /// # Errors
    ///
    /// [`ChecksError::Access`] where `repository` does not exist or `caller` holds no read on it.
    fn last_green(
        &self,
        caller: &Caller,
        repository: RepositoryId,
    ) -> impl Future<Output = Result<Option<Address>, ChecksError>> + Send;

    /// Cancels `job` of `repository`, queued or running, and returns it as it now stands.
    ///
    /// # Errors
    ///
    /// [`ChecksError::Access`] where `caller` may not read `repository` or may not administer the
    /// runners of its organisation, and [`ChecksError::Queue`] with [`JobQueueError::NotFound`]
    /// where the organisation has no such job, or [`JobQueueError::Finished`] where it has
    /// ended.
    fn cancel(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        job: JobId,
    ) -> impl Future<Output = Result<Job, ChecksError>> + Send;
}

/// Why a repository's checks were not read or changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ChecksError {
    /// The caller may not do what it asked.
    Access(AccessError),
    /// The job queue refused or failed.
    Queue(JobQueueError),
    /// The changeset log failed.
    Log(ChangesetLogError),
}

impl fmt::Display for ChecksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ChecksError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
        }
    }
}

impl From<AccessError> for ChecksError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<JobQueueError> for ChecksError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ChangesetLogError> for ChecksError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}
