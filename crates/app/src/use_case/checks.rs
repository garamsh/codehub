//! The checks driving port, served by the job queue, the changeset log, the repository directory
//! and the authorisation decision it is given.

use std::sync::Arc;

use crate::domain::address::Address;
use crate::domain::draft::DraftId;
use crate::domain::job::{Job, JobId};
use crate::domain::name::DraftName;
use crate::domain::principal::Caller;
use crate::domain::repository::RepositoryId;
use crate::port::driven::authorisation_decision::{Action, AuthorisationDecision, Resource};
use crate::port::driven::changeset_log::{ChangesetLog, Ref};
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driving::checks::{Checks, ChecksError, Plan};
use crate::use_case::access::{readable_repository, require};
use crate::use_case::plan::{PlanPorts, plan};
use crate::use_case::post_land_checks::PostLandChecks;

/// The driven ports the checks are built over.
pub struct ChecksAdapters {
    /// Holds the jobs.
    pub queue: Arc<dyn JobQueue>,
    /// Stores changesets and `last-green`.
    pub log: Arc<dyn ChangesetLog>,
    /// Finds a repository's organisation.
    pub directory: Arc<dyn RepositoryDirectory>,
    /// Says what callers may do.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Stores blobs.
    pub objects: Arc<dyn ObjectStore>,
    /// Stores derived data.
    pub derived: Arc<dyn DerivedData>,
    /// Reads the check definitions.
    pub definitions: Arc<dyn CheckDefinitions>,
    /// Stores drafts.
    pub drafts: Arc<dyn DraftStore>,
}

/// A repository's checks over the driven ports the composition site passes to it. A blocking job
/// that times out where the checks are read has its changeset reverted.
pub struct ChecksService {
    queue: Arc<dyn JobQueue>,
    log: Arc<dyn ChangesetLog>,
    directory: Arc<dyn RepositoryDirectory>,
    authorisation: Arc<dyn AuthorisationDecision>,
    objects: Arc<dyn ObjectStore>,
    derived: Arc<dyn DerivedData>,
    definitions: Arc<dyn CheckDefinitions>,
    drafts: Arc<dyn DraftStore>,
    checks: Arc<PostLandChecks>,
}

impl ChecksService {
    /// Checks over `adapters`, with the timeouts and reverts of `checks`.
    pub fn new(adapters: ChecksAdapters, checks: Arc<PostLandChecks>) -> Self {
        let ChecksAdapters {
            queue,
            log,
            directory,
            authorisation,
            objects,
            derived,
            definitions,
            drafts,
        } = adapters;
        Self {
            queue,
            log,
            directory,
            authorisation,
            objects,
            derived,
            definitions,
            drafts,
            checks,
        }
    }
}

#[async_trait::async_trait]
impl Checks for ChecksService {
    async fn jobs_of(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        changeset: Address,
        now: i64,
    ) -> Result<Vec<Job>, ChecksError> {
        readable_repository(
            &*self.directory,
            &*self.authorisation,
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
            &*self.directory,
            &*self.authorisation,
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
            &*self.directory,
            &*self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        require(
            &*self.authorisation,
            caller.principal(),
            Action::AdministerRunners,
            &Resource::Organisation(organisation.clone()),
        )
        .await?;
        let cancelled = self.queue.cancel(&organisation, job).await?;
        self.checks
            .resume(cancelled.repository, cancelled.changeset)
            .await
            .map_err(ChecksError::Revert)?;
        Ok(cancelled)
    }

    async fn plan(
        &self,
        caller: &Caller,
        repository: RepositoryId,
        draft: &DraftName,
    ) -> Result<Plan, ChecksError> {
        readable_repository(
            &*self.directory,
            &*self.authorisation,
            caller.principal(),
            repository,
        )
        .await?;
        let draft = DraftId {
            principal: caller.principal().clone(),
            name: draft.clone(),
        };
        let ports = PlanPorts {
            queue: Arc::clone(&self.queue),
            log: Arc::clone(&self.log),
            objects: Arc::clone(&self.objects),
            derived: Arc::clone(&self.derived),
            definitions: Arc::clone(&self.definitions),
            drafts: Arc::clone(&self.drafts),
        };
        Ok(plan(&ports, repository, &draft).await?)
    }
}
