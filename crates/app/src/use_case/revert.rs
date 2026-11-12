//! Reverting a landed changeset whose blocking check failed or timed out (ADR 0035).

use std::collections::BTreeMap;

use crate::domain::changeset::{Change, Changeset};
use crate::domain::draft::Draft;
use crate::domain::job::Job;
use crate::domain::revert::{self, AUTHOR, Revert};
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::revert::RevertError;
use crate::port::driving::source_control::{LandError, Landing};
use crate::use_case::checked_source_control::{CheckedAdapters, queue_checks};
use crate::use_case::land::land;
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::read_file::file_at;

/// The reverting of changesets over the driven ports a landing and the queuing of its checks use,
/// with the number of times a revert that lost the compare-and-swap on `main` is tried.
pub struct Reverts<'a, O, D, L, R, Q, C> {
    pub(crate) adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
    attempts: LandingAttempts,
}

impl<'a, O, D, L, R, Q, C> Reverts<'a, O, D, L, R, Q, C> {
    /// Reverts over `adapters`, each revert tried at most `attempts` times.
    pub const fn new(
        adapters: CheckedAdapters<'a, O, D, L, R, Q, C>,
        attempts: LandingAttempts,
    ) -> Self {
        Self { adapters, attempts }
    }
}

impl<O, D, L, R, Q, C> Reverts<'_, O, D, L, R, Q, C>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
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
    ///
    /// # Errors
    ///
    /// [`RevertError::Land`] with [`LandError::MainMoved`] where `main` moved under every
    /// attempt, and otherwise the failure of a port.
    pub(crate) async fn revert_on_blocking_failure(
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
            queue_checks(adapters, repository, landed)
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
