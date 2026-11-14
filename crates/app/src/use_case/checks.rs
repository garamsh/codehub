//! The checks driving port, served by the job queue, the changeset log, the repository directory
//! and the authorisation decision it is given.

use crate::domain::address::Address;
use crate::domain::job::{Job, JobId};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::checks::{Checks, ChecksError};
use crate::use_case::access::{readable_repository, require};
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::post_land_checks::{PostLandChecks, PostLandChecksAdapters};

/// The driven ports the checks are built over.
pub struct ChecksAdapters<'a, Q, L, R, A, O, D, C> {
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Stores changesets and `last-green`.
    pub log: &'a L,
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores derived data.
    pub derived: &'a D,
    /// Reads the check definitions.
    pub definitions: &'a C,
}

/// A repository's checks over the driven ports the composition site passes to it. A blocking job
/// that times out where the checks are read has its changeset reverted.
pub struct ChecksService<'a, Q, L, R, A, O, D, C> {
    queue: &'a Q,
    log: &'a L,
    directory: &'a R,
    authorisation: &'a A,
    checks: PostLandChecks<'a, O, D, L, R, Q, C>,
}

impl<'a, Q, L, R, A, O, D, C> ChecksService<'a, Q, L, R, A, O, D, C> {
    /// Checks over `adapters`, each revert tried at most `attempts` times.
    pub const fn new(
        adapters: ChecksAdapters<'a, Q, L, R, A, O, D, C>,
        attempts: LandingAttempts,
    ) -> Self {
        let ChecksAdapters {
            queue,
            log,
            directory,
            authorisation,
            objects,
            derived,
            definitions,
        } = adapters;
        Self {
            queue,
            log,
            directory,
            authorisation,
            checks: PostLandChecks::new(
                PostLandChecksAdapters {
                    objects,
                    derived,
                    log,
                    directory,
                    queue,
                    definitions,
                },
                attempts,
            ),
        }
    }
}

impl<Q, L, R, A, O, D, C> Checks for ChecksService<'_, Q, L, R, A, O, D, C>
where
    Q: JobQueue + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    C: CheckDefinitions + Sync,
{
    async fn jobs_of(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        now: i64,
    ) -> Result<Vec<Job>, ChecksError> {
        readable_repository(
            self.directory,
            self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        self.checks.expire(now).await?;
        Ok(self.queue.of_changeset(repository, changeset).await?)
    }

    async fn reference(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChecksError> {
        readable_repository(
            self.directory,
            self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        Ok(self.log.read_ref(repository, name).await?)
    }

    async fn cancel(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        job: JobId,
    ) -> Result<Job, ChecksError> {
        let (organisation, _) = readable_repository(
            self.directory,
            self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        require(
            self.authorisation,
            caller.principal(),
            Action::AdministerRunners,
            &Resource::Organisation(organisation.clone()),
        )
        .await?;
        Ok(self.queue.cancel(&organisation, job).await?)
    }
}
