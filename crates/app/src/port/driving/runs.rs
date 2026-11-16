//! Runs, as the principal who starts one sees them: started in a workspace, read, cancelled and
//! listed by output (ADR 0051).

use std::collections::BTreeSet;
use std::fmt;

use crate::domain::check::Command;
use crate::domain::file::File;
use crate::domain::name::DraftName;
use crate::domain::path::{Path, Prefix};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::domain::run::{Run, RunId};
use crate::port::driven::cache_refs::CacheRefsError;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::check_definitions::CheckFileError;
use crate::port::driven::derived_data::DerivedDataError;
use crate::port::driven::draft_store::DraftStoreError;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::{Grant, ObjectStoreError};
use crate::port::driving::access::AccessError;

/// What a caller asks to run.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StartRun {
    /// What the shell runs.
    pub command: Command,
    /// The directory the files sent are limited to; `None` sends the whole workspace.
    pub prefix: Option<Prefix>,
    /// The paths, under the prefix where one is given, whose files the run's changes may add.
    pub add: BTreeSet<Path>,
    /// The paths whose files, and the cache paths, make the run's output tree.
    pub outputs: BTreeSet<Path>,
}

/// One file of a run's output tree, with permission to download it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct OutputFile {
    /// The file's path in the repository.
    pub path: Path,
    /// The file.
    pub file: File,
    /// Permission to download its content.
    pub download: Grant,
}

/// What the principal who started runs asks of them. Every operation on a run answers
/// [`RunsError::NotFound`] to a caller who is not the principal that started it.
#[async_trait::async_trait]
pub trait Runs: Send + Sync {
    /// Starts a run of `request` in the caller's workspace `draft` of `repository`, and returns it
    /// queued.
    ///
    /// # Errors
    ///
    /// [`RunsError::Access`] where the repository is not readable, where the caller lacks `run` on
    /// it, or lacks `read` on a path the run is sent, on the run file, or on the repository where
    /// a cache ref would be seeded from `main`'s. [`RunsError::Drafts`] with not found where
    /// there is no such workspace. [`RunsError::AddOutsidePrefix`] where an `add` path is not
    /// under the prefix. [`RunsError::RunFile`] where the run file does not parse. No run is
    /// queued on any of these.
    async fn start(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
        request: StartRun,
    ) -> Result<Run, RunsError>;

    /// The run `run` of the caller.
    ///
    /// # Errors
    ///
    /// [`RunsError::NotFound`] where there is no such run of the caller in `repository`.
    async fn get(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
    ) -> Result<Run, RunsError>;

    /// The latest run that has ended of the caller's workspace `draft`.
    ///
    /// # Errors
    ///
    /// [`RunsError::Drafts`] with not found where there is no such workspace;
    /// [`RunsError::NoRun`] where none of its runs has ended.
    async fn latest(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Run, RunsError>;

    /// Cancels the run `run` of the caller and returns it.
    ///
    /// # Errors
    ///
    /// [`RunsError::NotFound`] where there is no such run of the caller in `repository`;
    /// [`RunsError::Finished`] where it has ended.
    async fn cancel(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
    ) -> Result<Run, RunsError>;

    /// The files of the output tree of the run `run` of the caller under `prefix`, each with a
    /// download grant. A run has an output tree only once a runner reported its result, so that
    /// until then the answer is empty.
    ///
    /// # Errors
    ///
    /// [`RunsError::NotFound`] where there is no such run of the caller in `repository`.
    async fn files(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        run: RunId,
        prefix: &Prefix,
    ) -> Result<Vec<OutputFile>, RunsError>;
}

/// Why a run was refused or not answered.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunsError {
    /// The caller may not start a run, or the repository is not readable.
    Access(AccessError),
    /// There is no such run for the caller; another principal's run is not told apart.
    NotFound(RunId),
    /// The run has ended and cannot be cancelled.
    Finished(RunId),
    /// None of the workspace's runs has ended.
    NoRun(DraftName),
    /// An `add` path is not under the prefix.
    AddOutsidePrefix(Path),
    /// The run file does not parse.
    RunFile {
        /// The run file's path.
        file: Path,
        /// What is wrong with it.
        error: CheckFileError,
    },
    /// The draft store failed or has no such workspace.
    Drafts(DraftStoreError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The object store failed.
    Objects(ObjectStoreError),
    /// The changeset log failed.
    Log(ChangesetLogError),
    /// The cache refs failed.
    CacheRefs(CacheRefsError),
    /// The job queue failed.
    Queue(JobQueueError),
}

impl fmt::Display for RunsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Access(error) => write!(f, "{error}"),
            Self::NotFound(run) => write!(f, "no run {run}"),
            Self::Finished(run) => write!(f, "run {run} has ended"),
            Self::NoRun(draft) => write!(f, "no run of {draft} has ended"),
            Self::AddOutsidePrefix(path) => write!(f, "{path} is not under the prefix"),
            Self::RunFile { file, error } => write!(f, "{file}: {error}"),
            Self::Drafts(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Objects(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::CacheRefs(error) => write!(f, "{error}"),
            Self::Queue(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for RunsError {}

impl From<AccessError> for RunsError {
    fn from(error: AccessError) -> Self {
        Self::Access(error)
    }
}

impl From<DraftStoreError> for RunsError {
    fn from(error: DraftStoreError) -> Self {
        Self::Drafts(error)
    }
}

impl From<DerivedDataError> for RunsError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ObjectStoreError> for RunsError {
    fn from(error: ObjectStoreError) -> Self {
        Self::Objects(error)
    }
}

impl From<ChangesetLogError> for RunsError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<CacheRefsError> for RunsError {
    fn from(error: CacheRefsError) -> Self {
        Self::CacheRefs(error)
    }
}

impl From<JobQueueError> for RunsError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}
