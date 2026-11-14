//! The post-land checks of a landing on `main` (ADR 0035): queuing its checks, expiring jobs,
//! settling an ended job and reverting the changeset of a blocking failure.

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::{Change, Changeset};
use crate::domain::check::{CheckDefinition, CheckKey, needs_fault};
use crate::domain::draft::Draft;
use crate::domain::job::{Job, JobState, NewJob, Resolution, empty_output_tree};
use crate::domain::path::{Path, Prefix};
use crate::domain::repository::RepositoryId;
use crate::domain::revert::{self, AUTHOR, Revert};
use crate::domain::tree::{Entry, Tree};
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::{DerivedData, DerivedDataError};
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::jobs::JobsError;
use crate::port::driving::revert::{QueueChecksError, RevertError};
use crate::port::driving::source_control::{LandError, Landing};
use crate::use_case::land::land;
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::last_green::{advance, blocking_checks_passed};
use crate::use_case::read_file::file_at;
use crate::use_case::source_control::AfterLanding;

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
    /// Derived data failed.
    Derived(DerivedDataError),
    /// A blocking check ended in failure and its changeset was not reverted.
    Revert(RevertError),
}

impl fmt::Display for SettleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Revert(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SettleError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Log(error) => Some(error),
            Self::Derived(error) => Some(error),
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

impl From<DerivedDataError> for SettleError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ProgressError> for SettleError {
    fn from(error: ProgressError) -> Self {
        match error {
            ProgressError::Queue(error) => Self::Queue(error),
            ProgressError::Derived(error) => Self::Derived(error),
            ProgressError::Log(error) => Self::Log(error),
        }
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
            SettleError::Derived(error) => Self::Derived(error),
            SettleError::Revert(error) => Self::Revert(error),
        }
    }
}

/// Why the waiting checks of a changeset were not resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProgressError {
    /// The job queue failed.
    Queue(JobQueueError),
    /// Derived data failed.
    Derived(DerivedDataError),
    /// The changeset log failed.
    Log(ChangesetLogError),
}

impl fmt::Display for ProgressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Queue(error) => write!(f, "{error}"),
            Self::Derived(error) => write!(f, "{error}"),
            Self::Log(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ProgressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Queue(error) => Some(error),
            Self::Derived(error) => Some(error),
            Self::Log(error) => Some(error),
        }
    }
}

impl From<JobQueueError> for ProgressError {
    fn from(error: JobQueueError) -> Self {
        Self::Queue(error)
    }
}

impl From<DerivedDataError> for ProgressError {
    fn from(error: DerivedDataError) -> Self {
        Self::Derived(error)
    }
}

impl From<ChangesetLogError> for ProgressError {
    fn from(error: ChangesetLogError) -> Self {
        Self::Log(error)
    }
}

impl From<ProgressError> for QueueChecksError {
    fn from(error: ProgressError) -> Self {
        match error {
            ProgressError::Queue(error) => Self::Queue(error),
            ProgressError::Derived(error) => Self::Derived(error),
            ProgressError::Log(error) => Self::Log(error),
        }
    }
}

impl From<ProgressError> for RevertError {
    fn from(error: ProgressError) -> Self {
        match error {
            ProgressError::Queue(error) => Self::Queue(error),
            ProgressError::Derived(error) => Self::Derived(error),
            ProgressError::Log(error) => Self::Log(error),
        }
    }
}

/// The driven ports the post-land checks are built over.
pub struct PostLandChecksAdapters<'a, O, D, L, R, Q, C> {
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores derived data.
    pub derived: &'a D,
    /// Stores changesets and `last-green`.
    pub log: &'a L,
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Reads the check definitions.
    pub definitions: &'a C,
}

/// The post-land checks over the driven ports a landing and the queuing of its checks use, with
/// the number of times a revert that lost the compare-and-swap on `main` is tried.
pub struct PostLandChecks<'a, O, D, L, R, Q, C> {
    adapters: PostLandChecksAdapters<'a, O, D, L, R, Q, C>,
    attempts: LandingAttempts,
}

impl<'a, O, D, L, R, Q, C> PostLandChecks<'a, O, D, L, R, Q, C> {
    /// Post-land checks over `adapters`, each revert tried at most `attempts` times.
    pub const fn new(
        adapters: PostLandChecksAdapters<'a, O, D, L, R, Q, C>,
        attempts: LandingAttempts,
    ) -> Self {
        Self { adapters, attempts }
    }
}

impl<O, D, L, R, Q, C> AfterLanding for PostLandChecks<'_, O, D, L, R, Q, C>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    /// Records the checks `landed` defines in `repository` and queues those that need nothing,
    /// unless their key passed already, or moves `last-green` to it where none blocks. Where its
    /// definitions cannot be read, the failure is recorded as a failed blocking
    /// job and `landed` is reverted, with `timestamp` as the revert's time.
    ///
    /// # Errors
    ///
    /// [`AfterLandingError::Queue`] where the checks were not queued, and
    /// [`AfterLandingError::Revert`] where the revert failed.
    async fn after_landing(
        &self,
        repository: RepositoryId,
        landed: Address,
        timestamp: i64,
    ) -> Result<(), AfterLandingError> {
        let unreadable = self
            .queue(repository, landed)
            .await
            .map_err(AfterLandingError::Queue)?;
        if let Some(failed) = unreadable {
            self.revert_on_blocking_failure(&failed, timestamp)
                .await
                .map_err(AfterLandingError::Revert)?;
        }
        Ok(())
    }
}

impl<O, D, L, R, Q, C> PostLandChecks<'_, O, D, L, R, Q, C>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    /// Acts on time having passed to `now`: expires the queue's jobs and, for each job that timed
    /// out, resolves the checks of its changeset that wait for it and, where it blocks, reverts the
    /// changeset with `now` as the revert's time.
    ///
    /// # Errors
    ///
    /// The failure of the job queue, or of a revert.
    pub async fn expire(&self, now: i64) -> Result<(), RevertError> {
        for job in self.adapters.queue.expire(now).await? {
            self.resume(job.repository, job.changeset).await?;
            if job.check.blocking {
                self.revert_on_blocking_failure(&job, now).await?;
            }
        }
        Ok(())
    }

    /// What `ended`, a job that has just ended, means for its changeset: the checks that wait for
    /// it are queued, passed from the cache or skipped, a blocking failure reverts the changeset
    /// with `now` as the revert's time, and `last-green` advances where every blocking check of the
    /// changeset has passed.
    ///
    /// # Errors
    ///
    /// The failure of the job queue, derived data or the changeset log, or of a revert.
    pub async fn settle(&self, ended: &Job, now: i64) -> Result<(), SettleError> {
        self.resume(ended.repository, ended.changeset).await?;
        match ended.state {
            JobState::Failed | JobState::TimedOut => {
                if ended.check.blocking {
                    self.revert_on_blocking_failure(ended, now).await?;
                }
            }
            JobState::Waiting
            | JobState::Queued
            | JobState::Running { .. }
            | JobState::Passed
            | JobState::Cancelled
            | JobState::Skipped
            | JobState::Cached => {}
        }
        Ok(())
    }

    /// Settles every waiting check of `changeset` in `repository` whose needs have ended, and
    /// repeats while that settles more, then moves `last-green` to `changeset` where every blocking
    /// check has passed.
    ///
    /// A check whose needs have all passed is keyed: where its key passed in the repository it is
    /// recorded as passed from the cache, with the output tree of the run that passed; otherwise it
    /// is queued. A check any of whose needs failed, timed out, was cancelled or was skipped is
    /// recorded as skipped. A check with no needs is settled where it was recorded, which is how
    /// the checks without needs are queued together.
    ///
    /// # Errors
    ///
    /// The failure of the job queue, derived data or the changeset log.
    pub async fn resume(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<(), RevertError> {
        self.progress(repository, changeset).await?;
        Ok(())
    }

    async fn progress(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<(), ProgressError> {
        let adapters = &self.adapters;
        let mut files = None;
        loop {
            let jobs = adapters.queue.of_changeset(repository, changeset).await?;
            let mut settled = false;
            for job in jobs.iter().filter(|job| job.state == JobState::Waiting) {
                let Some(resolution) = self.resolution(job, &jobs, &mut files, changeset).await?
                else {
                    continue;
                };
                settled |= adapters.queue.resolve(job.id, resolution).await?;
            }
            if !settled {
                if blocking_checks_passed(&jobs) {
                    advance(adapters.log, repository, changeset).await?;
                }
                return Ok(());
            }
        }
    }

    /// What becomes of the waiting `job` among `jobs`, the jobs of its changeset; `None` where a
    /// check it needs has not ended. `files` holds the files of the changeset's tree once they
    /// have been read.
    async fn resolution(
        &self,
        job: &Job,
        jobs: &[Job],
        files: &mut Option<Vec<(Path, Address)>>,
        changeset: Address,
    ) -> Result<Option<Resolution>, ProgressError> {
        let adapters = &self.adapters;
        let empty = empty_output_tree();
        let mut needed = Vec::new();
        for id in job.check.needed() {
            let found = jobs.iter().find(|other| other.check.id() == id);
            match found {
                Some(other) if !other.state.is_finished() => return Ok(None),
                Some(other) if other.state.is_passed() => {
                    if let Some(key) = other.key {
                        needed.push((key, other.output_tree.unwrap_or(empty)));
                        continue;
                    }
                    return Ok(Some(Resolution::Skip));
                }
                Some(_) | None => return Ok(Some(Resolution::Skip)),
            }
        }
        let tree = match files {
            Some(files) => files,
            None => {
                let root = adapters.derived.root(job.repository, changeset).await?;
                files.insert(self.input_files(job.repository, root).await?)
            }
        };
        let inputs: Vec<(Path, Address)> = tree
            .iter()
            .filter(|(path, _)| job.check.reads(path))
            .cloned()
            .collect();
        let key = CheckKey::new(&job.check, &inputs, &needed);
        Ok(Some(
            match adapters.queue.passed_with_key(job.repository, key).await? {
                Some(output_tree) => Resolution::Cache { key, output_tree },
                None => Resolution::Queue { key },
            },
        ))
    }

    /// The path and content address of every file of the tree rooted at `root`, by path.
    async fn input_files(
        &self,
        repository: RepositoryId,
        root: Address,
    ) -> Result<Vec<(Path, Address)>, DerivedDataError> {
        let derived = self.adapters.derived;
        let mut files = Vec::new();
        let mut pending = vec![(Vec::new(), root)];
        while let Some((directory, address)) = pending.pop() {
            for (name, entry) in derived.tree(repository, address).await?.entries {
                let mut components = directory.clone();
                components.push(name);
                match entry {
                    Entry::Directory(subtree) => pending.push((components, subtree)),
                    Entry::File(file) => {
                        let path = Path::new(components)
                            .map_err(|error| DerivedDataError::Backend(error.to_string()))?;
                        files.push((path, file.content));
                    }
                }
            }
        }
        files.sort();
        Ok(files)
    }

    /// Records the checks `landed` defines as waiting and resolves them. Returns the failed job recorded where the definitions could not be read, which the caller is to
    /// revert for; `None` otherwise.
    async fn queue(
        &self,
        repository: RepositoryId,
        landed: Address,
    ) -> Result<Option<Job>, QueueChecksError> {
        let adapters = &self.adapters;
        let (organisation, _) = adapters
            .directory
            .names_of(repository)
            .await?
            .ok_or(QueueChecksError::NoRepository(repository))?;
        let job = |check: CheckDefinition| NewJob {
            organisation: organisation.clone(),
            repository,
            changeset: landed,
            check,
        };

        let file = adapters.definitions.file();
        let root = adapters.derived.root(repository, landed).await?;
        let mut definitions = Vec::new();
        for (folder, content) in self.check_files(repository, root).await? {
            let bytes = adapters.objects.get(repository, content).await?;
            match adapters.definitions.parse(&folder, &bytes) {
                Ok(read) => definitions.extend(read),
                Err(error) => {
                    let unreadable = job(CheckDefinition::unreadable(&folder, file));
                    let failed = adapters
                        .queue
                        .record_failed(unreadable, error.to_string().into_bytes())
                        .await?;
                    return Ok(Some(failed));
                }
            }
        }
        if let Some(fault) = needs_fault(&definitions) {
            let unreadable = job(CheckDefinition::unreadable(&fault.check().folder, file));
            let failed = adapters
                .queue
                .record_failed(unreadable, fault.to_string().into_bytes())
                .await?;
            return Ok(Some(failed));
        }

        adapters
            .queue
            .queue(definitions.into_iter().map(job).collect())
            .await?;
        self.progress(repository, landed).await?;
        Ok(None)
    }

    /// The folders of the tree rooted at `root` that hold a check definitions file, each with the
    /// address of the file's content, a folder before the folders under it.
    async fn check_files(
        &self,
        repository: RepositoryId,
        root: Address,
    ) -> Result<Vec<(Prefix, Address)>, DerivedDataError> {
        let derived = self.adapters.derived;
        let file = self.adapters.definitions.file();
        let mut found = Vec::new();
        let mut pending = vec![(Prefix::whole(), root)];
        while let Some((folder, address)) = pending.pop() {
            let tree = derived.tree(repository, address).await?;
            for (name, entry) in &tree.entries {
                if let Entry::Directory(subtree) = entry {
                    let mut components = folder.components().to_vec();
                    components.push(name.clone());
                    pending.push((Prefix::new(components), *subtree));
                }
            }
            if let Some(content) = file_in(derived, repository, &tree, file).await? {
                found.push((folder, content));
            }
        }
        found.sort_by(|(first, _), (second, _)| first.cmp(second));
        Ok(found)
    }

    /// Where the blocking check `job` has failed or timed out, reverts its changeset on `main`:
    /// lands a changeset authored by the server that undoes what the changeset changed against its
    /// first parent, through the landing merge, with `timestamp` as its time. It is called wherever
    /// a blocking check ends in failure or in a timeout, or its definitions could not be read.
    ///
    /// The revert is reserved in the job queue before anything lands, and the reservation is kept
    /// where the revert then fails, so a changeset is reverted at most once. It is recorded with
    /// `job`: as landed, or as conflicting with later changes, which lands nothing, or as failed
    /// with the reason where reading or landing it fails, after which the error is returned, or
    /// [`RevertError::Unrecorded`] with both where recording the failure fails too. A
    /// landed revert has its own checks queued. Nothing is reverted where another job of the
    /// changeset holds the reservation, where the changeset is itself a revert the server made, or
    /// where `job` did not fail or time out.
    async fn revert_on_blocking_failure(
        &self,
        job: &Job,
        timestamp: i64,
    ) -> Result<(), RevertError> {
        let adapters = &self.adapters;
        let repository = job.repository;
        let Some(message) = revert::message(job.changeset, &job.check.name, job.state) else {
            return Ok(());
        };
        let changeset = adapters.log.get(repository, job.changeset).await?;
        if revert::is_automatic(&changeset) {
            return Ok(());
        }
        if !adapters.queue.reserve_revert(job.id).await? {
            return Ok(());
        }

        let recorded = match self.land_revert(job, changeset, message, timestamp).await {
            Ok(recorded) => recorded,
            Err(error) => {
                let revert = Revert::failed(&error);
                return Err(match adapters.queue.record_revert(job.id, revert).await {
                    Ok(_) => error,
                    Err(recording) => RevertError::Unrecorded {
                        error: Box::new(error),
                        recording,
                    },
                });
            }
        };

        adapters
            .queue
            .record_revert(job.id, recorded.clone())
            .await?;
        if let Revert::Landed(landed) = recorded {
            // The revert is never reverted in its turn, so a failed unreadable-definitions job it
            // leaves behind needs no further action.
            self.queue(repository, landed)
                .await
                .map_err(RevertError::ChecksNotQueued)?;
        }
        Ok(())
    }

    /// Lands the changeset that undoes `changeset`, which `job` reserved the revert of, and
    /// returns what came of it. Nothing is recorded.
    async fn land_revert(
        &self,
        job: &Job,
        changeset: Changeset,
        message: String,
        timestamp: i64,
    ) -> Result<Revert, RevertError> {
        let adapters = &self.adapters;
        let repository = job.repository;
        let parent_root = match changeset.parents.first() {
            Some(parent) => Some(adapters.derived.root(repository, *parent).await?),
            None => None,
        };
        let root = Some(adapters.derived.root(repository, job.changeset).await?);
        let mut before = BTreeMap::new();
        let mut after = BTreeMap::new();
        for path in changeset.changes.keys() {
            if let Some(file) = file_at(adapters.derived, repository, parent_root, path).await? {
                before.insert(path.clone(), file);
            }
            if let Some(file) = file_at(adapters.derived, repository, root, path).await? {
                after.insert(path.clone(), file);
            }
        }
        let undo: BTreeMap<_, Change> = revert::inverse(&before, &after);

        let staged = Draft {
            base: Some(job.changeset),
            description: String::new(),
            changes: undo,
        };
        let landing = Landing { message, timestamp };
        match land(
            adapters.objects,
            adapters.log,
            adapters.derived,
            repository,
            AUTHOR,
            staged,
            landing,
            self.attempts,
        )
        .await
        {
            Ok(landed) => Ok(Revert::Landed(landed)),
            Err(LandError::Conflicts(conflicts)) => {
                Ok(Revert::Conflicted(conflicts.into_keys().collect()))
            }
            Err(error) => Err(RevertError::Land(error)),
        }
    }
}

/// The address of the content of the file at `path` below `tree`, if it holds one.
async fn file_in<D: DerivedData>(
    derived: &D,
    repository: RepositoryId,
    tree: &Tree,
    path: &Path,
) -> Result<Option<Address>, DerivedDataError> {
    let Some((first, rest)) = path.components().split_first() else {
        return Ok(None);
    };
    match tree.entries.get(first) {
        Some(Entry::File(file)) if rest.is_empty() => Ok(Some(file.content)),
        Some(Entry::Directory(subtree)) => match Path::new(rest.to_vec()) {
            Ok(rest) => Ok(file_at(derived, repository, Some(*subtree), &rest)
                .await?
                .map(|file| file.content)),
            Err(_) => Ok(None),
        },
        Some(Entry::File(_)) | None => Ok(None),
    }
}
