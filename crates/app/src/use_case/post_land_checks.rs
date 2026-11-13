//! The post-land checks of a landing on `main` (ADR 0035): queuing its checks, expiring jobs,
//! settling an ended job and reverting the changeset of a blocking failure.

use std::collections::BTreeMap;
use std::fmt;

use crate::domain::address::Address;
use crate::domain::changeset::{Change, Changeset};
use crate::domain::check::CheckDefinition;
use crate::domain::draft::Draft;
use crate::domain::job::{Job, JobState, NewJob};
use crate::domain::repository::RepositoryId;
use crate::domain::revert::{self, AUTHOR, Revert};
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::changeset_log::ChangesetLogError;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::job_queue::JobQueueError;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::jobs::JobsError;
use crate::port::driving::revert::{QueueChecksError, RevertError};
use crate::port::driving::source_control::{LandError, Landing};
use crate::use_case::checked_source_control::CheckedAdapters;
use crate::use_case::land::land;
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::last_green::{advance, blocking_checks_passed};
use crate::use_case::read_file::file_at;

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

/// The post-land checks over the driven ports a landing and the queuing of its checks use, with
/// the number of times a revert that lost the compare-and-swap on `main` is tried.
pub struct PostLandChecks<'a, O, D, L, R, Q, C> {
    adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
    attempts: LandingAttempts,
}

impl<'a, O, D, L, R, Q, C> PostLandChecks<'a, O, D, L, R, Q, C> {
    /// Post-land checks over `adapters`, each revert tried at most `attempts` times.
    pub const fn new(
        adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
        attempts: LandingAttempts,
    ) -> Self {
        Self { adapters, attempts }
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
    /// Queues the checks `landed` defines in `repository`, or moves `last-green` to it where none
    /// blocks. Where its definitions cannot be read, the failure is recorded as a failed blocking
    /// job and `landed` is reverted, with `timestamp` as the revert's time.
    ///
    /// # Errors
    ///
    /// [`AfterLandingError::Queue`] where the checks were not queued, and
    /// [`AfterLandingError::Revert`] where the revert failed.
    pub async fn after_landing(
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

    /// Acts on time having passed to `now`: expires the queue's jobs and, for each blocking job
    /// that timed out, reverts its changeset with `now` as the revert's time.
    ///
    /// # Errors
    ///
    /// The failure of the job queue, or of a revert.
    pub async fn expire(&self, now: i64) -> Result<(), RevertError> {
        for job in self.adapters.queue.expire(now).await? {
            if job.check.blocking {
                self.revert_on_blocking_failure(&job, now).await?;
            }
        }
        Ok(())
    }

    /// What `ended`, a job that has just ended, means for its changeset: a blocking failure
    /// reverts the changeset with `now` as the revert's time, and a pass that leaves every
    /// blocking check of the changeset passed advances `last-green`.
    ///
    /// # Errors
    ///
    /// The failure of the job queue or of the changeset log, or of a revert.
    pub async fn settle(&self, ended: &Job, now: i64) -> Result<(), SettleError> {
        match ended.state {
            JobState::Failed | JobState::TimedOut => {
                if ended.check.blocking {
                    self.revert_on_blocking_failure(ended, now).await?;
                }
            }
            JobState::Passed => {
                let jobs = self
                    .adapters
                    .queue
                    .of_changeset(ended.repository, ended.changeset)
                    .await?;
                if blocking_checks_passed(&jobs) {
                    advance(self.adapters.log, ended.repository, ended.changeset).await?;
                }
            }
            JobState::Queued | JobState::Running { .. } | JobState::Cancelled => {}
        }
        Ok(())
    }

    /// Queues the checks `landed` defines, or moves `last-green` to it where none blocks. Returns
    /// the failed job recorded where the definitions could not be read, which the caller is to
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
        let definitions = match file_at(adapters.derived, repository, Some(root), file).await? {
            None => Vec::new(),
            Some(found) => {
                let bytes = adapters.objects.get(repository, found.content).await?;
                match adapters.definitions.parse(&bytes) {
                    Ok(definitions) => definitions,
                    Err(error) => {
                        let unreadable = job(CheckDefinition::unreadable(file));
                        let failed = adapters
                            .queue
                            .record_failed(unreadable, error.to_string().into_bytes())
                            .await?;
                        return Ok(Some(failed));
                    }
                }
            }
        };

        let blocking = definitions.iter().any(|check| check.blocking);
        adapters
            .queue
            .queue(definitions.into_iter().map(job).collect())
            .await?;
        if !blocking {
            advance(adapters.log, repository, landed).await?;
        }
        Ok(None)
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
