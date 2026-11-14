//! Checks, as a reader of a repository sees them: the jobs of a changeset, its refs, cancelling a
//! job, and the plan of a draft.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::check::{CheckId, Pattern};
use crate::domain::job::{Job, JobId};
use crate::domain::name::DraftName;
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::changeset_log::{ChangesetLogError, Ref};
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::draft_store::DraftStoreError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::ObjectStoreError;
use crate::port::driving::access::AccessError;
use crate::port::driving::revert::RevertError;
use crate::port::driving::source_control::LandError;

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

    /// The changeset the ref `name` points at, `main` or `last-green`; `None` before it is first
    /// set.
    ///
    /// # Errors
    ///
    /// [`ChecksError::Access`] where `repository` does not exist or `caller` holds no read on it.
    fn reference(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        name: Ref,
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

    /// The plan of the caller's draft `draft` in `repository`: every check its tree would define,
    /// in `needs` order, each either run or taken from the cache, with keys computed against the
    /// tree as if the draft landed now.
    ///
    /// # Errors
    ///
    /// [`ChecksError::Access`] where `repository` does not exist or `caller` holds no read on it,
    /// and [`ChecksError::Plan`] where the caller holds no such draft, the draft conflicts with
    /// `main`, or the definitions it would land cannot be read.
    fn plan(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> impl Future<Output = Result<Plan, ChecksError>> + Send;
}

/// What landing a draft now would do about each check, in `needs` order.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    /// The checks; a check comes after every check it needs.
    pub checks: Vec<PlannedCheck>,
}

/// One check of a [`Plan`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PlannedCheck {
    /// The check.
    pub check: CheckId,
    /// Whether a failure would revert the landing.
    pub blocking: bool,
    /// The patterns that select the files the check reads.
    pub inputs: Vec<Pattern>,
    /// What would become of the check.
    pub outcome: PlanOutcome,
}

/// What would become of a check.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PlanOutcome {
    /// A run of the check's key has passed; the check takes that result.
    Cached,
    /// The check would run, for each of these reasons.
    Run(Vec<RunReason>),
}

/// Why a check would run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunReason {
    /// The draft changes a file the check reads.
    InputsChanged,
    /// A check it needs would run, so its key is not known until that one has run.
    NeedRuns(CheckId),
    /// Its key is known and no run of it has passed.
    NoPassedRun,
}

/// Why a draft's plan was not made.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum PlanError {
    /// Rebasing the draft onto `main` failed, for a conflict or for a port.
    Rebase(LandError),
    /// The draft store holds no such draft, or failed.
    Drafts(DraftStoreError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The job queue failed.
    Queue(JobQueueError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// The definitions the draft would land cannot be read: the file that declares them is not
    /// readable, or its checks name each other wrongly.
    Definitions {
        /// The file at fault, from the repository root.
        file: String,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rebase(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Definitions { file, reason } => write!(f, "{file}: {reason}"),
        }
    }
}

impl std::error::Error for PlanError {}

impl From<LandError> for PlanError {
    fn from(error: LandError) -> Self {
        Self::Rebase(error)
    }
}

impl From<DraftStoreError> for PlanError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

impl From<DerivedDataError> for PlanError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ObjectStoreError> for PlanError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<JobQueueError> for PlanError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<ChangesetLogError> for PlanError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
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
    /// A blocking job timed out and its changeset was not reverted.
    Revert(RevertError),
    /// A draft's plan was not made.
    Plan(PlanError),
}

impl fmt::Display for ChecksError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
            Self::Plan(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ChecksError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Access(error) => Some(error),
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Revert(error) => Some(error),
            Self::Plan(error) => Some(error),
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

impl From<RevertError> for ChecksError {
    fn from(error: RevertError) -> Self {
        Self::Revert(error)
    }
}

impl From<PlanError> for ChecksError {
    fn from(error: PlanError) -> Self {
        Self::Plan(error)
    }
}
