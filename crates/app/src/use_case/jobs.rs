//! The jobs driving port, served by the runner registry, the job queue, the object store, derived
//! data and the changeset log it is given.

use crate::domain::address::Address;
use crate::domain::job::{Heartbeat, Job, JobId, JobResult, JobState, output_tail};
use crate::domain::path::{Component, Path};
use crate::domain::principal::{Caller, Principal};
use crate::domain::repository::RepositoryId;
use crate::domain::runner::{Registration, RegistrationId};
use crate::domain::tree::Entry;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::{JobQueue, JobQueueError};
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::runner_registry::RunnerRegistry;
use crate::port::driving::jobs::{Claimed, ClaimedFile, Jobs, JobsError};
use crate::use_case::last_green::{advance, blocking_checks_passed};
use crate::use_case::revert::revert_on_blocking_failure;

/// The driven ports the jobs are built over.
pub struct JobsAdapters<'a, G, Q, O, D, L> {
    /// Finds the callers' registrations.
    pub registry: &'a G,
    /// Holds the jobs.
    pub queue: &'a Q,
    /// Stores blobs.
    pub objects: &'a O,
    /// Stores derived data.
    pub derived: &'a D,
    /// Stores changesets and `last-green`.
    pub log: &'a L,
}

/// Jobs served to registered runners, each claim holding its job for `lease` seconds.
pub struct JobsService<'a, G, Q, O, D, L> {
    registry: &'a G,
    queue: &'a Q,
    objects: &'a O,
    derived: &'a D,
    log: &'a L,
    lease: i64,
}

impl<'a, G, Q, O, D, L> JobsService<'a, G, Q, O, D, L> {
    /// Jobs over `adapters`, where a claim or a heartbeat holds a job for `lease` seconds.
    pub const fn new(adapters: JobsAdapters<'a, G, Q, O, D, L>, lease: i64) -> Self {
        let JobsAdapters {
            registry,
            queue,
            objects,
            derived,
            log,
        } = adapters;
        Self {
            registry,
            queue,
            objects,
            derived,
            log,
            lease,
        }
    }
}

/// Acts on time having passed to `now`: expires the queue's jobs and, for each blocking job that
/// timed out, calls [`revert_on_blocking_failure`].
pub(crate) async fn expire<Q: JobQueue>(queue: &Q, now: i64) -> Result<(), JobQueueError> {
    for job in queue.expire(now).await? {
        if job.check.blocking {
            revert_on_blocking_failure(job.repository, &job);
        }
    }
    Ok(())
}

impl<G, Q, O, D, L> JobsService<'_, G, Q, O, D, L>
where
    G: RunnerRegistry + Sync,
{
    /// The registrations of `principal`, which are at least one.
    async fn registrations(&self, principal: &Principal) -> Result<Vec<Registration>, JobsError> {
        let registrations = self.registry.of_principal(principal).await?;
        if registrations.is_empty() {
            Err(JobsError::NotRegistered)
        } else {
            Ok(registrations)
        }
    }
}

impl<G, Q, O, D, L> JobsService<'_, G, Q, O, D, L>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
{
    /// Every file of the tree of `changeset`, with a grant to download it.
    async fn files(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<ClaimedFile>, JobsError> {
        let mut files = Vec::new();
        let root = self.derived.root(repository, changeset).await?;
        let mut pending: Vec<(Vec<Component>, Address)> = vec![(Vec::new(), root)];
        while let Some((directory, tree)) = pending.pop() {
            for (name, entry) in self.derived.tree(repository, tree).await?.entries {
                let mut components = directory.clone();
                components.push(name);
                match entry {
                    Entry::Directory(subtree) => pending.push((components, subtree)),
                    Entry::File(file) => {
                        let size = self.objects.get(repository, file.content).await?.len();
                        files.push(ClaimedFile {
                            path: Path::new(components)?,
                            kind: file.kind,
                            size: u64::try_from(size).unwrap_or(u64::MAX),
                            download: self
                                .objects
                                .download_grant(repository, file.content)
                                .await?,
                        });
                    }
                }
            }
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }
}

fn ids(registrations: &[Registration]) -> Vec<RegistrationId> {
    registrations.iter().map(|registration| registration.id).collect()
}

impl<G, Q, O, D, L> Jobs for JobsService<'_, G, Q, O, D, L>
where
    G: RunnerRegistry + Sync,
    Q: JobQueue + Sync,
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
{
    async fn claim(&self, caller: &Caller, now: i64) -> Result<Option<Claimed>, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        expire(self.queue, now).await?;
        let Some(job) = self.queue.claim(&registrations, now, self.lease).await? else {
            return Ok(None);
        };
        let files = self.files(job.repository, job.changeset).await?;
        Ok(Some(Claimed { job, files }))
    }

    async fn heartbeat(
        &self,
        caller: &Caller,
        job: JobId,
        now: i64,
    ) -> Result<Heartbeat, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        expire(self.queue, now).await?;
        Ok(self
            .queue
            .heartbeat(job, &ids(&registrations), now, self.lease)
            .await?)
    }

    async fn report(
        &self,
        caller: &Caller,
        job: JobId,
        result: JobResult,
        now: i64,
    ) -> Result<(), JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        expire(self.queue, now).await?;
        let result = JobResult {
            output: output_tail(&result.output),
            ..result
        };
        let ended = self
            .queue
            .complete(job, &ids(&registrations), result)
            .await?;
        settle(self.queue, self.log, &ended).await
    }
}

/// What an ended job means for its changeset: a blocking failure calls
/// [`revert_on_blocking_failure`], and a pass that leaves every blocking check of the changeset
/// passed advances `last-green`.
async fn settle<Q: JobQueue, L: ChangesetLog>(
    queue: &Q,
    log: &L,
    ended: &Job,
) -> Result<(), JobsError> {
    match ended.state {
        JobState::Failed | JobState::TimedOut => {
            if ended.check.blocking {
                revert_on_blocking_failure(ended.repository, ended);
            }
        }
        JobState::Passed => {
            let jobs = queue.of_changeset(ended.repository, ended.changeset).await?;
            if blocking_checks_passed(&jobs) {
                advance(log, ended.repository, ended.changeset).await?;
            }
        }
        JobState::Queued | JobState::Running { .. } | JobState::Cancelled => {}
    }
    Ok(())
}
