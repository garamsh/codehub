//! The failures of the post-land checks (ADR 0035), shared by the driving ports that act on a
//! landing or on a blocking check ending in failure: the checks could not be queued, the changeset
//! could not be reverted, or an ended job could not be settled.

use std::fmt;

use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::ObjectStoreError;
use crate::port::driven::repository_directory::DirectoryError;
use crate::port::driving::jobs::JobsError;
use crate::port::driving::source_control::LandError;

/// Why the checks of a landed changeset were not queued.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum QueueChecksError {
    /// The repository directory failed.
    Directory(DirectoryError),
    /// The directory knows no such repository.
    NoRepository(RepositoryId),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The job queue refused or failed.
    Queue(JobQueueError),
    /// The changeset log failed.
    Log(ChangesetLogError),
}

impl fmt::Display for QueueChecksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(error) => write!(f, "{error}"),
            Self::NoRepository(repository) => write!(f, "no repository {repository}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for QueueChecksError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Directory(error) => Some(error),
            Self::NoRepository(_) => None,
            Self::Derived(error) => Some(error),
            Self::Objects(error) => Some(error),
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
        }
    }
}

impl From<DirectoryError> for QueueChecksError {
    fn from(error: DirectoryError) -> Self {
        Self::Directory(error)
    }
}

impl From<DerivedDataError> for QueueChecksError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ObjectStoreError> for QueueChecksError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<JobQueueError> for QueueChecksError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ChangesetLogError> for QueueChecksError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

/// Why the post-land checks of a landing failed after it landed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum AfterLandingError {
    /// The checks were not queued.
    Queue(QueueChecksError),
    /// The definitions could not be read and the changeset was not reverted.
    Revert(RevertError),
}

impl fmt::Display for AfterLandingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for AfterLandingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Revert(error) => Some(error),
        }
    }
}

/// Why an ended job was not settled.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SettleError {
    /// The job queue failed.
    Queue(JobQueueError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// A blocking check ended in failure and its changeset was not reverted.
    Revert(RevertError),
}

impl fmt::Display for SettleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SettleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Revert(error) => Some(error),
        }
    }
}

impl From<JobQueueError> for SettleError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ChangesetLogError> for SettleError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<RevertError> for SettleError {
    fn from(error: RevertError) -> Self {
        Self::Revert(error)
    }
}

impl From<SettleError> for JobsError {
    fn from(error: SettleError) -> Self {
        match error {
            SettleError::Queue(error) => Self::Queue(error),
            SettleError::Log(error) => Self::Log(error),
            SettleError::Revert(error) => Self::Revert(error),
        }
    }
}

/// Why the revert of a changeset whose blocking check ended in failure failed. The check's result
/// is recorded all the same.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RevertError {
    /// The job queue refused or failed.
    Queue(JobQueueError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The revert could not be landed, for a reason other than a conflict;
    /// [`LandError::MainMoved`] where `main` kept moving through every attempt.
    Land(LandError),
    /// The revert landed and `main` advanced, but its checks were not queued; carries why.
    ChecksNotQueued(QueueChecksError),
    /// The revert failed after its reservation and the failure could not be recorded with the job,
    /// so the job shows no revert although the changeset will not be tried again; carries the
    /// failure of the revert and the failure of the recording.
    Unrecorded {
        /// Why the revert failed.
        error: Box<RevertError>,
        /// Why the failure could not be recorded.
        recording: JobQueueError,
    },
}

impl fmt::Display for RevertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Land(error) => write!(f, "revert not landed: {error}"),
            Self::ChecksNotQueued(cause) => write!(f, "revert's checks not queued: {cause}"),
            Self::Unrecorded { error, recording } => {
                write!(f, "{error}; the failure was not recorded: {recording}")
            }
        }
    }
}

impl std::error::Error for RevertError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Derived(error) => Some(error),
            Self::Land(error) => Some(error),
            Self::ChecksNotQueued(_) => None,
            Self::Unrecorded { error, .. } => Some(error.as_ref()),
        }
    }
}

impl From<JobQueueError> for RevertError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ChangesetLogError> for RevertError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<DerivedDataError> for RevertError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}
