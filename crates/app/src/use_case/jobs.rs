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
use crate::port::driven::check_definitions::CheckDefinitions;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::job_queue::JobQueue;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driven::runner_registry::RunnerRegistry;
use crate::port::driving::jobs::{Claimed, ClaimedFile, Jobs, JobsError};
use crate::port::driving::revert::RevertError;
use crate::use_case::checked_source_control::CheckedAdapters;
use crate::use_case::landing_attempts::LandingAttempts;
use crate::use_case::last_green::{advance, blocking_checks_passed};
use crate::use_case::revert::Reverts;

/// The driven ports the jobs are built over.
pub struct JobsAdapters<'a, G, Q, O, D, L, R, C> {
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
    /// Finds a repository's organisation.
    pub directory: &'a R,
    /// Reads the check definitions.
    pub definitions: &'a C,
}

/// Jobs served to registered runners, each claim holding its job for `lease` seconds. A blocking
/// job that ends in failure or in a timeout has its changeset reverted.
pub struct JobsService<'a, G, Q, O, D, L, R, C> {
    registry: &'a G,
    queue: &'a Q,
    objects: &'a O,
    derived: &'a D,
    reverts: Reverts<'a, O, D, L, R, Q, C>,
    lease: i64,
}

impl<'a, G, Q, O, D, L, R, C> JobsService<'a, G, Q, O, D, L, R, C> {
    /// Jobs over `adapters`, where a claim or a heartbeat holds a job for `lease` seconds and each
    /// revert is tried at most `attempts` times.
    pub const fn new(
        adapters: JobsAdapters<'a, G, Q, O, D, L, R, C>,
        lease: i64,
        attempts: LandingAttempts,
    ) -> Self {
        let JobsAdapters {
            registry,
            queue,
            objects,
            derived,
            log,
            directory,
            definitions,
        } = adapters;
        Self {
            registry,
            queue,
            objects,
            derived,
            reverts: Reverts::new(
                CheckedAdapters {
                    objects,
                    derived,
                    log,
                    directory,
                    queue,
                    definitions,
                },
                attempts,
            ),
            lease,
        }
    }
}

/// Acts on time having passed to `now`: expires the queue's jobs and, for each blocking job that
/// timed out, reverts its changeset with `now` as the revert's time.
pub(crate) async fn expire<O, D, L, R, Q, C>(
    reverts: &Reverts<'_, O, D, L, R, Q, C>,
    now: i64,
) -> Result<(), RevertError>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    for job in reverts.adapters.queue.expire(now).await? {
        if job.check.blocking {
            reverts.revert_on_blocking_failure(&job, now).await?;
        }
    }
    Ok(())
}

impl<G, Q, O, D, L, R, C> JobsService<'_, G, Q, O, D, L, R, C>
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

impl<G, Q, O, D, L, R, C> JobsService<'_, G, Q, O, D, L, R, C>
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
                        files.push(ClaimedFile {
                            path: Path::new(components)?,
                            kind: file.kind,
                            address: file.content,
                            size: self.objects.size(repository, file.content).await?,
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
    registrations
        .iter()
        .map(|registration| registration.id)
        .collect()
}

impl<G, Q, O, D, L, R, C> Jobs for JobsService<'_, G, Q, O, D, L, R, C>
where
    G: RunnerRegistry + Sync,
    Q: JobQueue + Sync,
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    C: CheckDefinitions + Sync,
{
    async fn claim(&self, caller: &Caller, now: i64) -> Result<Option<Claimed>, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        expire(&self.reverts, now).await?;
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
        expire(&self.reverts, now).await?;
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
        expire(&self.reverts, now).await?;
        let result = JobResult {
            output: output_tail(&result.output),
            ..result
        };
        let ended = self
            .queue
            .complete(job, &ids(&registrations), result)
            .await?;
        settle(&self.reverts, &ended, now).await
    }
}

/// What an ended job means for its changeset: a blocking failure reverts the changeset with `now`
/// as the revert's time, and a pass that leaves every blocking check of the changeset passed
/// advances `last-green`.
async fn settle<O, D, L, R, Q, C>(
    reverts: &Reverts<'_, O, D, L, R, Q, C>,
    ended: &Job,
    now: i64,
) -> Result<(), JobsError>
where
    O: ObjectStore + Sync,
    D: DerivedData + Sync,
    L: ChangesetLog + Sync,
    R: RepositoryDirectory + Sync,
    Q: JobQueue + Sync,
    C: CheckDefinitions + Sync,
{
    match ended.state {
        JobState::Failed | JobState::TimedOut => {
            if ended.check.blocking {
                reverts.revert_on_blocking_failure(ended, now).await?;
            }
        }
        JobState::Passed => {
            let adapters = &reverts.adapters;
            let jobs = adapters
                .queue
                .of_changeset(ended.repository, ended.changeset)
                .await?;
            if blocking_checks_passed(&jobs) {
                advance(adapters.log, ended.repository, ended.changeset).await?;
            }
        }
        JobState::Queued | JobState::Running { .. } | JobState::Cancelled => {}
    }
    Ok(())
}
