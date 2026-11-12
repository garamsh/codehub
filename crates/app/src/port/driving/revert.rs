//! The failure shared by the driving ports that act on a blocking check ending in failure: the
//! changeset could not be reverted, or the revert's own checks could not be queued.

use std::fmt;

use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driving::source_control::LandError;

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
    ChecksNotQueued(String),
}

impl fmt::Display for RevertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Land(error) => write!(f, "revert not landed: {error}"),
            Self::ChecksNotQueued(cause) => write!(f, "revert's checks not queued: {cause}"),
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
