//! The checks driving port, served by the job queue, the changeset log, the repository directory
//! and the authorisation decision it is given.

use crate::domain::address::Address;
use crate::domain::job::{Job, JobId};
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::checks::{Checks, ChecksError};
use crate::use_case::access::{readable_repository, require};
use crate::use_case::jobs::expire;

/// The driven ports the checks are built over.
pub struct ChecksAdapters<'a, Q, L, R, A> {
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Stores changesets and `last-green`.
    pub log: &'a L,
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Says what callers may do.
    pub authorisation: &'a A,
}

/// A repository's checks over the driven ports the composition site passes to it.
pub struct ChecksService<'a, Q, L, R, A> {
    queue: &'a Q,
    log: &'a L,
    directory: &'a R,
    authorisation: &'a A,
}

impl<'a, Q, L, R, A> ChecksService<'a, Q, L, R, A> {
    /// Checks over `adapters`.
    pub const fn new(adapters: ChecksAdapters<'a, Q, L, R, A>) -> Self {
        let ChecksAdapters {
            queue,
            log,
            directory,
            authorisation,
        } = adapters;
        Self {
            queue,
            log,
            directory,
            authorisation,
        }
    }
}

impl<Q, L, R, A> Checks for ChecksService<'_, Q, L, R, A>
where
    Q: JobQueue + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    A: AuthorisationDecision + Sync,
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
        expire(self.queue, now).await?;
        Ok(self.queue.of_changeset(repository, changeset).await?)
    }

    async fn last_green(
        &self,
        caller: &Caller,
        repository: RepositoryId,
    ) -> Result<Option<Address>, ChecksError> {
        readable_repository(
            self.directory,
            self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        Ok(self.log.read_ref(repository, Ref::LastGreen).await?)
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
