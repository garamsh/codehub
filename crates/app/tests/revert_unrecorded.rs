//! A revert that fails after its reservation and whose failure cannot be recorded says both.

#[cfg(test)]
pub mod common;

use adapter_memory::MemoryJobQueue;
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::check::CheckKey;
use app::domain::job::{
    CacheRef, Heartbeat, Job, JobId, JobResult, NewJob, Outcome, Produced, Resolution,
};
use app::domain::name::OrganisationName;
use app::domain::path::{Path, Prefix};
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driving::jobs::{Jobs, JobsError};
use app::port::driving::revert::RevertError;
use app::port::driving::source_control::LandError;
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use common::Harness;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::{
    RunnerDeployment, check, definitions, land, organisation, runner, text,
};

const LOG_DOWN: &str = "log down";
const RECORDING_DOWN: &str = "queue is down";

/// A log that fails every move of `main` and otherwise is the one it wraps.
struct FailsMainSwap<'a>(&'a adapter_memory::MemoryChangesetLog);

impl ChangesetLog for FailsMainSwap<'_> {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        self.0.append(repository, changeset).await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        self.0.get(repository, id).await
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        self.0.read_ref(repository, name).await
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        if name == Ref::Main {
            return Err(ChangesetLogError::Backend(LOG_DOWN.to_owned()));
        }
        self.0
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

/// A queue that refuses to record a failed revert and does everything else as [`MemoryJobQueue`]
/// does.
struct RecordRefusingQueue<'a>(&'a MemoryJobQueue);

impl JobQueue for RecordRefusingQueue<'_> {
    async fn queue(&self, jobs: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError> {
        self.0.queue(jobs).await
    }

    async fn resolve(&self, job: JobId, resolution: Resolution) -> Result<bool, JobQueueError> {
        self.0.resolve(job, resolution).await
    }

    async fn passed_with_key(
        &self,
        repository: RepositoryId,
        key: CheckKey,
    ) -> Result<Option<Address>, JobQueueError> {
        self.0.passed_with_key(repository, key).await
    }

    async fn record_failed(&self, job: NewJob, output: Vec<u8>) -> Result<Job, JobQueueError> {
        self.0.record_failed(job, output).await
    }

    async fn claim(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Job>, JobQueueError> {
        self.0.claim(registrations, now, lease).await
    }

    async fn heartbeat(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError> {
        self.0.heartbeat(job, runners, now, lease).await
    }

    async fn running(&self, job: JobId, runners: &[RegistrationId]) -> Result<Job, JobQueueError> {
        self.0.running(job, runners).await
    }

    async fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
        output_tree: Address,
    ) -> Result<Job, JobQueueError> {
        self.0.complete(job, runners, result, output_tree).await
    }

    async fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> Result<Job, JobQueueError> {
        self.0.cancel(organisation, job).await
    }

    async fn reserve_revert(&self, job: JobId) -> Result<bool, JobQueueError> {
        self.0.reserve_revert(job).await
    }

    async fn record_revert(&self, job: JobId, revert: Revert) -> Result<Job, JobQueueError> {
        if matches!(revert, Revert::Failed { .. }) {
            return Err(JobQueueError::Backend(RECORDING_DOWN.to_owned()));
        }
        self.0.record_revert(job, revert).await
    }

    async fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<Job>, JobQueueError> {
        self.0.of_changeset(repository, changeset).await
    }

    async fn cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
    ) -> Result<Option<CacheRef>, JobQueueError> {
        self.0.cache_ref(repository, folder, path).await
    }

    async fn advance_cache_ref(
        &self,
        repository: RepositoryId,
        folder: &Prefix,
        path: &Path,
        from: Option<Address>,
        to: CacheRef,
    ) -> Result<bool, JobQueueError> {
        self.0
            .advance_cache_ref(repository, folder, path, from, to)
            .await
    }

    async fn expire(&self, now: i64) -> Result<Vec<Job>, JobQueueError> {
        self.0.expire(now).await
    }
}

#[tokio::test]
async fn a_failed_revert_that_cannot_be_recorded_returns_both_failures() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", "1\n")]).await.unwrap();
    let specs = [check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    let bad = land(&repo, &ada, "bad", &[(&file, &body), ("a", "2\n")])
        .await
        .unwrap();
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job.id;
    let log = FailsMainSwap(&harness.log);
    let queue = RecordRefusingQueue(&harness.queue);
    let service = JobsService::new(
        JobsAdapters {
            registry: &harness.registry,
            queue: &queue,
            objects: &harness.objects,
            derived: &harness.derived,
            log: &log,
            directory: &harness.directory,
            definitions: &harness.definitions,
        },
        30,
        LandingAttempts::default(),
    );
    let result = JobResult {
        outcome: Outcome::Failed,
        exit_code: Some(1),
        output: Vec::new(),
    };

    let error = service
        .report(&bot, job, result, Produced::default(), 5)
        .await
        .unwrap_err();

    let JobsError::Revert(RevertError::Unrecorded { error, recording }) = &error else {
        panic!("expected Unrecorded, got {error:?}");
    };
    assert_eq!(
        **error,
        RevertError::Land(LandError::Log(ChangesetLogError::Backend(
            LOG_DOWN.to_owned()
        )))
    );
    assert_eq!(
        *recording,
        JobQueueError::Backend(RECORDING_DOWN.to_owned())
    );
    assert_eq!(repo.main().await, Some(bad));
}
