//! Jobs, as a registered runner sees them: claimed, kept alive and reported.

use std::fmt;

use crate::domain::address::Address;
use crate::domain::encoding::{DecodeError, EncodeError};
use crate::domain::file::FileKind;
use crate::domain::job::{Heartbeat, Job, JobId, JobResult, Produced};
use crate::domain::path::{Path, PathError};
use crate::domain::principal::Caller;
use crate::domain::run::{Run, RunId, RunReport};
use crate::domain::tree::DeriveError;
use crate::port::driven::cache_refs::CacheRefsError;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::draft_store::DraftStoreError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::{Grant, ObjectStoreError};
use crate::port::driven::runner_registry::RunnerRegistryError;
use crate::port::driving::access::AccessError;
use crate::port::driving::revert::RevertError;

/// What a registered runner asks of the application. Each operation is refused with
/// [`JobsError::NotRegistered`] where the caller is not registered as a runner. Each takes `now`,
/// in seconds since 1970-01-01T00:00:00Z.
#[async_trait::async_trait]
pub trait Jobs: Send + Sync {
    /// Takes the oldest queued job the caller's registrations match, or `None` where none is
    /// queued. The caller holds it for the service's lease, and keeps it by heartbeating.
    ///
    /// # Errors
    ///
    /// [`JobsError::NotRegistered`] where the caller is not registered as a runner.
    async fn claim(&self, caller: &Caller, now: i64) -> Result<Option<Claimed>, JobsError>;

    /// Takes the oldest queued run the caller's registrations may take, or `None` where none is
    /// queued. The caller holds it for the service's lease, and keeps it by heartbeating.
    ///
    /// # Errors
    ///
    /// [`JobsError::NotRegistered`] where the caller is not registered as a runner.
    async fn claim_run(&self, caller: &Caller, now: i64) -> Result<Option<ClaimedRun>, JobsError>;

    /// Extends the lease on `job`, which is a check's job or a run, which the caller holds, or
    /// tells it to stop.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    async fn heartbeat(
        &self,
        caller: &Caller,
        job: JobId,
        now: i64,
    ) -> Result<Heartbeat, JobsError>;

    /// Grants uploads of those of `files` the object store lacks, for `job`, which is a check's
    /// job or a run, which the caller holds. A file the store has is not granted.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    async fn upload_grants(
        &self,
        caller: &Caller,
        job: JobId,
        files: Vec<Upload>,
        now: i64,
    ) -> Result<Vec<UploadGrant>, JobsError>;

    /// Ends `job`, which the caller holds, as `result` says, with the files it left, which the
    /// caller has uploaded. A pass records the tree of the files under the check's `outputs`, and
    /// may advance the cache refs and move `last-green`. A pass whose files are not all in the
    /// object store ends as a failure.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `job`.
    async fn report(
        &self,
        caller: &Caller,
        job: JobId,
        result: JobResult,
        produced: Produced,
        now: i64,
    ) -> Result<(), JobsError>;

    /// Ends the run `run`, which the caller holds, as `report` says, whose files the caller has
    /// uploaded. Records the output tree of the files under the run's outputs and cache paths and
    /// writes the run's changes back to its workspace as ADR 0051 says; after a command that
    /// exited, moves the workspace's cache refs. A report that names a file the object store lacks
    /// ends the run as an executor failure. A run that was cancelled or timed out takes one
    /// report, which records its streams, object counts and output tree and writes nothing.
    ///
    /// # Errors
    ///
    /// [`JobsError::Queue`] with [`JobQueueError::NotFound`] where the caller does not hold `run`;
    /// [`JobsError::WrongKind`] where the caller holds a check's job of that number;
    /// [`JobsError::UnboundChanges`] where the report changes a path the run was neither sent nor
    /// allowed to add, or one under its cache paths; the run is then not ended.
    async fn report_run(
        &self,
        caller: &Caller,
        run: RunId,
        report: RunReport,
        now: i64,
    ) -> Result<(), JobsError>;
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

/// A run a runner took, and the files it is sent.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClaimedRun {
    /// The run, running.
    pub run: Run,
    /// The workspace files sent with the run, each at its path from the repository root.
    pub files: Vec<ClaimedFile>,
    /// The run's cache paths, each with the files of its workspace cache ref's tree.
    pub caches: Vec<ClaimedCache>,
}

/// A cache path of a run and the files of the tree its cache ref held when the run started.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ClaimedCache {
    /// The cache path, from the repository root.
    pub path: Path,
    /// The files of the tree, each at its path under the cache path.
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
    /// The cache refs failed.
    CacheRefs(CacheRefsError),
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
    /// A run's workspace could not be read, or the principal's access could not be decided.
    Access(AccessError),
    /// The draft store failed.
    Drafts(DraftStoreError),
    /// The caller reported a run's result for a check's job, or a check's result for a run.
    WrongKind(JobId),
    /// A run's report names changes to paths the run was neither sent nor allowed to add, or under
    /// one of its cache paths.
    UnboundChanges(Vec<Path>),
}

impl fmt::Display for JobsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotRegistered => f.write_str("the caller is not a registered runner"),
            Self::Queue(error) => write!(f, "{error}"),
            Self::CacheRefs(error) => write!(f, "{error}"),
            Self::Registry(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Path(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
            Self::Access(error) => write!(f, "{error}"),
            Self::Drafts(error) => write!(f, "{error}"),
            Self::WrongKind(job) => write!(f, "the report does not fit the kind of job {job}"),
            Self::UnboundChanges(paths) => write!(
                f,
                "the report changes paths the run was not sent or allowed to add: {}",
                paths
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }
}

impl std::error::Error for JobsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::NotRegistered | Self::WrongKind(_) | Self::UnboundChanges(_) => None,
            Self::Queue(error) => Some(error),
            Self::CacheRefs(error) => Some(error),
            Self::Registry(error) => Some(error),
            Self::Objects(error) => Some(error),
            Self::Derived(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Revert(error) => Some(error),
            Self::Access(error) => Some(error),
            Self::Drafts(error) => Some(error),
        }
    }
}

impl From<JobQueueError> for JobsError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<CacheRefsError> for JobsError {
    fn from(error: CacheRefsError) -> Self {
        Self::CacheRefs(error)
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

impl From<AccessError> for JobsError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<DraftStoreError> for JobsError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

impl From<RevertError> for JobsError {
    fn from(error: RevertError) -> Self {
        Self::Revert(error)
    }
}
