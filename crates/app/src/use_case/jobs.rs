//! The jobs driving port, served by the runner registry, the job queue, the cache refs, the object
//! store, derived data and the changeset log it is given.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::domain::address::Address;
use crate::domain::changeset::Change;
use crate::domain::encoding::DecodeError;
use crate::domain::file::File;
use crate::domain::job::{
    Heartbeat, Job, JobId, JobResult, Needs, Outcome, Produced, ProducedFile, empty_output_tree,
    output_tail,
};
use crate::domain::path::{Path, PathError, Prefix};
use crate::domain::principal::{Caller, Principal};
use crate::domain::repository::RepositoryId;
use crate::domain::run::{RunId, RunReport};
use crate::domain::runner::{Registration, RegistrationId};
use crate::domain::tree::{Tree, derive};
use crate::port::driven::authorisation_decision::AuthorisationDecision;
use crate::port::driven::cache_refs::CacheRefs;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::job_queue::{JobQueue, JobQueueError};
use crate::port::driven::object_store::{ObjectStore, ObjectStoreError};
use crate::port::driven::repository_directory::RepositoryDirectory;
use crate::port::driven::runner_registry::RunnerRegistry;
use crate::port::driving::jobs::{
    Claimed, ClaimedFile, ClaimedRun, Jobs, JobsError, Upload, UploadGrant,
};
use crate::use_case::cache_refs::advance;
use crate::use_case::post_land_checks::PostLandChecks;
use crate::use_case::run_jobs::RunJobs;
use crate::use_case::tree_checks::{Files, Resolved, resolve, walk_files};

/// The driven ports the jobs are built over.
pub struct JobsAdapters {
    /// Finds the callers' registrations.
    pub registry: Arc<dyn RunnerRegistry>,
    /// Holds the jobs.
    pub queue: Arc<dyn JobQueue>,
    /// Holds the cache refs.
    pub cache_refs: Arc<dyn CacheRefs>,
    /// Stores blobs.
    pub objects: Arc<dyn ObjectStore>,
    /// Stores derived data.
    pub derived: Arc<dyn DerivedData>,
    /// Stores changesets and `last-green`.
    pub log: Arc<dyn ChangesetLog>,
    /// Stores drafts, which a run's changes are written back to.
    pub drafts: Arc<dyn DraftStore>,
    /// Says what a run's principal may write.
    pub authorisation: Arc<dyn AuthorisationDecision>,
    /// Finds a repository's organisation and name.
    pub directory: Arc<dyn RepositoryDirectory>,
}

/// Jobs served to registered runners, each claim holding its job for `lease` seconds. A blocking
/// job that ends in failure or in a timeout has its changeset reverted. The jobs of runs are
/// served as ADR 0051 says.
pub struct JobsService {
    registry: Arc<dyn RunnerRegistry>,
    queue: Arc<dyn JobQueue>,
    cache_refs: Arc<dyn CacheRefs>,
    objects: Arc<dyn ObjectStore>,
    derived: Arc<dyn DerivedData>,
    log: Arc<dyn ChangesetLog>,
    checks: Arc<PostLandChecks>,
    runs: RunJobs,
    lease: i64,
}

impl JobsService {
    /// Jobs over `adapters`, where a claim or a heartbeat holds a job for `lease` seconds and the
    /// ending of a job is settled by `checks`.
    pub fn new(adapters: JobsAdapters, checks: Arc<PostLandChecks>, lease: i64) -> Self {
        let JobsAdapters {
            registry,
            queue,
            cache_refs,
            objects,
            derived,
            log,
            drafts,
            authorisation,
            directory,
        } = adapters;
        let runs = RunJobs::new(
            Arc::clone(&queue),
            Arc::clone(&cache_refs),
            Arc::clone(&objects),
            Arc::clone(&derived),
            drafts,
            authorisation,
            directory,
        );
        Self {
            registry,
            queue,
            cache_refs,
            objects,
            derived,
            log,
            checks,
            runs,
            lease,
        }
    }
}

impl JobsService {
    /// The registrations of `principal`, which are at least one.
    async fn registrations(&self, principal: &Principal) -> Result<Vec<Registration>, JobsError> {
        let registrations = self.registry.of_principal(principal).await?;
        if registrations.is_empty() {
            Err(JobsError::NotRegistered)
        } else {
            Ok(registrations)
        }
    }

    /// Acts on time having passed to `now`, for the checks' jobs and for the runs.
    async fn expire(&self, now: i64) -> Result<(), JobsError> {
        self.checks.expire(now).await?;
        Ok(self.queue.expire_runs(now).await?)
    }
}

/// `path` below `prefix`.
fn joined(prefix: &Prefix, path: &Path) -> Result<Path, PathError> {
    let mut components = prefix.components().to_vec();
    components.extend_from_slice(path.components());
    Path::new(components)
}

/// `path` relative to `prefix`; `None` where `prefix` does not cover it, or is the path itself.
pub(crate) fn relative(prefix: &Prefix, path: &Path) -> Option<Path> {
    let rest = path.components().strip_prefix(prefix.components())?;
    Path::new(rest.to_vec()).ok()
}

/// The files of `produced` that `prefixes` cover, by path.
pub(crate) fn covered(produced: &[ProducedFile], prefixes: &[Prefix]) -> BTreeMap<Path, File> {
    produced
        .iter()
        .filter(|file| prefixes.iter().any(|prefix| prefix.covers(&file.path)))
        .map(|file| (file.path.clone(), file.file))
        .collect()
}

/// Every file of the output or cache tree at `root`, a blob in the object store, by its path from
/// the root.
pub(crate) async fn walk_tree<E>(
    objects: &dyn ObjectStore,
    repository: RepositoryId,
    root: Address,
) -> Result<BTreeMap<Path, File>, E>
where
    E: From<ObjectStoreError> + From<DecodeError> + From<PathError>,
{
    walk_files(
        root,
        |address| async move {
            if address == empty_output_tree() {
                Ok(Tree::default())
            } else {
                Ok(Tree::decode(&objects.get(repository, address).await?)?)
            }
        },
        E::from,
        None,
    )
    .await
}

/// Stores the tree of `files`, and the trees below it, and returns its address.
pub(crate) async fn store_tree(
    objects: &dyn ObjectStore,
    repository: RepositoryId,
    files: BTreeMap<Path, File>,
) -> Result<Address, JobsError> {
    let changes = files
        .into_iter()
        .map(|(path, file)| (path, Change::File(file)))
        .collect();
    let derived = derive(&Tree::default(), &changes, &BTreeMap::new())?;
    for tree in derived.trees.values() {
        objects.put(repository, tree.encode()?).await?;
    }
    Ok(derived.root)
}

/// `file` at `path`, with its size and a grant to download it.
pub(crate) async fn claimed_file(
    objects: &dyn ObjectStore,
    repository: RepositoryId,
    path: Path,
    file: File,
) -> Result<ClaimedFile, ObjectStoreError> {
    Ok(ClaimedFile {
        path,
        kind: file.kind,
        address: file.content,
        size: objects.size(repository, file.content).await?,
        download: objects.download_grant(repository, file.content).await?,
    })
}

impl JobsService {
    /// The files `job` is granted, by path from the repository root.
    async fn granted(&self, job: &Job) -> Result<BTreeMap<Path, File>, JobsError> {
        let mut granted = BTreeMap::new();
        let caches = job.check.caches.iter().zip(job.cache_prefixes());
        for (declared, prefix) in caches {
            let Some(cache) = self
                .cache_refs
                .cache_ref(job.repository, &job.check.folder, declared)
                .await?
            else {
                continue;
            };
            for (path, file) in
                walk_tree::<JobsError>(&*self.objects, job.repository, cache.tree).await?
            {
                granted.insert(joined(&prefix, &path)?, file);
            }
        }
        let jobs = self
            .queue
            .of_changeset(job.repository, job.changeset)
            .await?;
        let root = self.derived.root(job.repository, job.changeset).await?;
        // A claimed job's needs have passed, so it is ready; a job that is not grants only its
        // caches.
        let files = Files::Landed {
            derived: &*self.derived,
            repository: job.repository,
            root,
        };
        if let Resolved::Ready(dependencies) =
            resolve(&files, &job.check, Needs::of(&job.check, &jobs)).await?
        {
            for (_, output) in dependencies.needed {
                granted
                    .extend(walk_tree::<JobsError>(&*self.objects, job.repository, output).await?);
            }
            granted.extend(dependencies.inputs);
        }
        Ok(granted)
    }

    /// The files `job` is granted, each with a grant to download it.
    async fn files(&self, job: &Job) -> Result<Vec<ClaimedFile>, JobsError> {
        let mut files = Vec::new();
        for (path, file) in self.granted(job).await? {
            files.push(claimed_file(&*self.objects, job.repository, path, file).await?);
        }
        Ok(files)
    }

    /// Whether every file of `produced` is in the object store.
    async fn stored(
        &self,
        repository: RepositoryId,
        produced: &Produced,
    ) -> Result<bool, JobsError> {
        for file in produced.outputs.iter().chain(&produced.caches) {
            if !self.objects.exists(repository, file.file.content).await? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Points each cache ref of `job` at the tree `job` left in its cache path, where `job`'s
    /// changeset is later than the one the ref records. Where a ref moves between the read and
    /// the compare-and-swap, decides again over where it moved.
    async fn advance_caches(
        &self,
        job: &Job,
        caches: &BTreeMap<Path, File>,
    ) -> Result<(), JobsError> {
        let declared = job.check.caches.iter().zip(job.cache_prefixes());
        for (path, prefix) in declared {
            let files = caches
                .iter()
                .filter_map(|(file, content)| Some((relative(&prefix, file)?, *content)))
                .collect();
            let tree = store_tree(&*self.objects, job.repository, files).await?;
            advance::<JobsError>(
                &*self.cache_refs,
                &*self.log,
                job.repository,
                &job.check.folder,
                path,
                job.changeset,
                tree,
            )
            .await?;
        }
        Ok(())
    }
}

fn ids(registrations: &[Registration]) -> Vec<RegistrationId> {
    registrations
        .iter()
        .map(|registration| registration.id)
        .collect()
}

#[async_trait::async_trait]
impl Jobs for JobsService {
    async fn claim(&self, caller: &Caller, now: i64) -> Result<Option<Claimed>, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        let Some(job) = self.queue.claim(&registrations, now, self.lease).await? else {
            return Ok(None);
        };
        let files = self.files(&job).await?;
        Ok(Some(Claimed { job, files }))
    }

    async fn claim_run(&self, caller: &Caller, now: i64) -> Result<Option<ClaimedRun>, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        self.runs.claim(&registrations, now, self.lease).await
    }

    async fn heartbeat(
        &self,
        caller: &Caller,
        job: JobId,
        now: i64,
    ) -> Result<Heartbeat, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        let runners = ids(&registrations);
        match self
            .queue
            .heartbeat_run(RunId::new(job.value()), &runners, now, self.lease)
            .await
        {
            Err(JobQueueError::NotFound(_)) => {}
            answer => return Ok(answer?),
        }
        Ok(self.queue.heartbeat(job, &runners, now, self.lease).await?)
    }

    async fn upload_grants(
        &self,
        caller: &Caller,
        job: JobId,
        files: Vec<Upload>,
        now: i64,
    ) -> Result<Vec<UploadGrant>, JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        let runners = ids(&registrations);
        let repository = match self.queue.held_run(RunId::new(job.value()), &runners).await {
            Ok(run) => run.request.repository,
            Err(JobQueueError::NotFound(_)) => self.queue.running(job, &runners).await?.repository,
            Err(error) => return Err(error.into()),
        };
        let mut grants = Vec::new();
        for file in files {
            if !self.objects.exists(repository, file.address).await? {
                grants.push(UploadGrant {
                    address: file.address,
                    grant: self
                        .objects
                        .upload_grant(repository, file.address, file.size)
                        .await?,
                });
            }
        }
        Ok(grants)
    }

    async fn report(
        &self,
        caller: &Caller,
        job: JobId,
        result: JobResult,
        produced: Produced,
        now: i64,
    ) -> Result<(), JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        let runners = ids(&registrations);
        let held = match self.queue.running(job, &runners).await {
            Err(JobQueueError::NotFound(_))
                if self
                    .queue
                    .held_run(RunId::new(job.value()), &runners)
                    .await
                    .is_ok() =>
            {
                return Err(JobsError::WrongKind(job));
            }
            held => held?,
        };
        let mut result = JobResult {
            output: output_tail(&result.output),
            ..result
        };
        if result.outcome == Outcome::Passed && !self.stored(held.repository, &produced).await? {
            result.outcome = Outcome::Failed;
            result.output.extend_from_slice(
                b"\ncodehub: a file the check left was not uploaded, so the check failed\n",
            );
            result.output = output_tail(&result.output);
        }
        let passed = result.outcome == Outcome::Passed;
        let output_tree = if passed {
            let outputs = covered(&produced.outputs, &held.output_prefixes());
            store_tree(&*self.objects, held.repository, outputs).await?
        } else {
            empty_output_tree()
        };
        let ended = self
            .queue
            .complete(job, &ids(&registrations), result, output_tree)
            .await?;
        if passed {
            let caches = covered(&produced.caches, &held.cache_prefixes());
            self.advance_caches(&ended, &caches).await?;
        }
        Ok(self.checks.settle(&ended, now).await?)
    }

    async fn report_run(
        &self,
        caller: &Caller,
        run: RunId,
        report: RunReport,
        now: i64,
    ) -> Result<(), JobsError> {
        let registrations = self.registrations(caller.principal()).await?;
        self.expire(now).await?;
        let runners = ids(&registrations);
        let job = JobId::new(run.value());
        let held = match self.queue.held_run(run, &runners).await {
            Err(JobQueueError::NotFound(_)) if self.queue.running(job, &runners).await.is_ok() => {
                return Err(JobsError::WrongKind(job));
            }
            held => held?,
        };
        self.runs
            .report(&runners, held, report, now, self.lease)
            .await
    }
}
