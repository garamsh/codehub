//! Jobs, as a registered runner sees them: claimed, kept alive and reported.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::encoding::{DecodeError, EncodeError};
use crate::domain::file::FileKind;
use crate::domain::job::{Heartbeat, Job, JobId, JobResult, Produced};
use crate::domain::path::{Path, PathError};
use crate::domain::principal::Caller;
use crate::domain::tree::DeriveError;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::{Grant, ObjectStoreError};
use crate::port::driven::runner_registry::RunnerRegistryError;
use crate::port::driving::revert::RevertError;

/// What a registered runner asks of the application. Each operation is refused with
/// [`JobsError::NotRegistered`] where the caller is not registered as a runner. Each takes `now`,
/// in seconds since 1970-01-01T00:00:00Z.
pub trait Jobs {
    /// Takes the oldest queued job the caller's registrations match, or `None` where none is
    /// queued. The caller holds it for the service's lease, and keeps it by heartbeating.
    ///
    /// # Errors
    ///
    /// [`JobsError::NotRegistered`] where the caller is not registered as a runner.
    fn claim(
        &self,
        caller: &Caller,
        now: i64,
    ) -> impl Future<Output = Result<Option<Claimed>, JobsError>> + Send;

    /// Extends the lease on `job`, which the caller holds, or tells it to stop.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    fn heartbeat(
        &self,
        caller: &Caller,
        job: JobId,
        now: i64,
    ) -> impl Future<Output = Result<Heartbeat, JobsError>> + Send;

    /// Grants uploads of those of `files` the object store lacks, for `job`, which the caller
    /// holds. A file the store has is not granted.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    fn upload_grants(
        &self,
        caller: &Caller,
        job: JobId,
        files: Vec<Upload>,
        now: i64,
    ) -> impl Future<Output = Result<Vec<UploadGrant>, JobsError>> + Send;

    /// Ends `job`, which the caller holds, as `result` says, with the files it left, which the
    /// caller has uploaded. A pass records the tree of the files under the check's `outputs`, and
    /// may advance the cache refs and move `last-green`. A pass whose files are not all in the
    /// object store ends as a failure.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    fn report(
        &self,
        caller: &Caller,
        job: JobId,
        result: JobResult,
        produced: Produced,
        now: i64,
    ) -> impl Future<Output = Result<(), JobsError>> + Send;
}

/// A file a runner wants to upload: the address of its bytes and their size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Upload {
    /// The address of the file's bytes.
    pub address: Address,
    /// The file's size in bytes.
    pub size: u64,
}

/// A grant to upload the bytes of one file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct UploadGrant {
    /// The address of the file's bytes.
    pub address: Address,
    /// A grant to upload them.
    pub grant: Grant,
}

/// A job a runner took, and the way to the files of the changeset it runs against.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Claimed {
    /// The job, running.
    pub job: Job,
    /// The files the job is granted: those its `inputs` match at the changeset, the output trees
    /// of the checks it needs and the trees of its cache refs. A file is listed once, at its path
    /// from the repository root; where more than one grant gives a path, the inputs win, then the
    /// outputs, then the caches.
    pub files: Vec<ClaimedFile>,
}

/// One file a runner may download.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClaimedFile {
    /// Where the file is in the tree.
    pub path: Path,
    /// The file's mode.
    pub kind: FileKind,
    /// The address of the file's bytes, which a runner checks what it downloads against.
    pub address: Address,
    /// The file's size in bytes.
    pub size: u64,
    /// A grant to download the file's bytes.
    pub download: Grant,
}

/// Why a runner's call was refused or failed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum JobsError {
    /// The caller is not registered as a runner.
    NotRegistered,
    /// The job queue refused or failed.
    Queue(JobQueueError),
    /// The runner registry failed.
    Registry(RunnerRegistryError),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// A file of the tree has no valid path, which the tree's derivation rules out.
    Path(PathError),
    /// A blocking check ended in failure and its changeset was not reverted.
    Revert(RevertError),
}

impl fmt::Display for JobsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered => f.write_str("the caller is not a registered runner"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::Registry(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Path(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for JobsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotRegistered => None,
            Self::Queue(error) => Some(error),
            Self::Registry(error) => Some(error),
            Self::Objects(error) => Some(error),
            Self::Derived(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Revert(error) => Some(error),
        }
    }
}

impl From<JobQueueError> for JobsError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<RunnerRegistryError> for JobsError {
    fn from(error: RunnerRegistryError) -> Self {
        Self::Registry(error)
    }
}

impl From<ObjectStoreError> for JobsError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<DerivedDataError> for JobsError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ChangesetLogError> for JobsError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<PathError> for JobsError {
    fn from(error: PathError) -> Self {
        Self::Path(error)
    }
}

/// A stored tree of outputs or of a cache that is not a tree is a fault of the object store.
impl From<DecodeError> for JobsError {
    fn from(error: DecodeError) -> Self {
        Self::Objects(ObjectStoreError::Backend(error.to_string()))
    }
}

/// A tree of outputs or of a cache that has no encoding is a fault of the derived data it is
/// recorded with.
impl From<EncodeError> for JobsError {
    fn from(error: EncodeError) -> Self {
        Self::Derived(DerivedDataError::Backend(error.to_string()))
    }
}

/// The files a job left that do not make a tree are a fault of the derived data they are
/// recorded with.
impl From<DeriveError> for JobsError {
    fn from(error: DeriveError) -> Self {
        Self::Derived(DerivedDataError::Backend(error.to_string()))
    }
}

impl From<RevertError> for JobsError {
    fn from(error: RevertError) -> Self {
        Self::Revert(error)
    }
}
