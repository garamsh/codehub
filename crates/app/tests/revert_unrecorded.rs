//! A revert that fails after its reservation and whose failure cannot be recorded says both.

#[cfg(test)]
pub mod common;

use std::sync::Arc;

use adapter_memory::MemoryJobQueue;
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::check::CheckKey;
use app::domain::draft::Instance;
use app::domain::job::{Heartbeat, Job, JobId, JobResult, NewJob, Outcome, Produced, Resolution};
use app::domain::name::{DraftName, OrganisationName};
use app::domain::principal::Principal;
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::run::{NewRun, Run, RunId, RunResult};
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driving::jobs::{Jobs, JobsError};
use app::port::driving::revert::RevertError;
use app::port::driving::source_control::LandError;
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::post_land_checks::{PostLandChecks, PostLandChecksAdapters};
use common::Harness;
use test_support::contract::Deployment;
use test_support::contract::runner_deployment::{
    RunnerDeployment, check, definitions, land, organisation, runner, text,
};

const LOG_DOWN: &str = "log down";
const RECORDING_DOWN: &str = "queue is down";

/// A log that fails every move of `main` and otherwise is the one it wraps.
struct FailsMainSwap(Arc<adapter_memory::MemoryChangesetLog>);

#[async_trait::async_trait]
impl ChangesetLog for FailsMainSwap {
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

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        self.0
            .first_parent_chain(repository, head, base, limit)
            .await
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
struct RecordRefusingQueue(Arc<MemoryJobQueue>);

#[async_trait::async_trait]
impl JobQueue for RecordRefusingQueue {
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

    async fn queue_run(&self, run: NewRun) -> Result<Run, JobQueueError> {
        self.0.queue_run(run).await
    }

    async fn run(&self, run: RunId) -> Result<Run, JobQueueError> {
        self.0.run(run).await
    }

    async fn cancel_run(&self, run: RunId) -> Result<Run, JobQueueError> {
        self.0.cancel_run(run).await
    }

    async fn latest_run(
        &self,
        repository: RepositoryId,
        principal: &Principal,
        draft: &DraftName,
        instance: Instance,
    ) -> Result<Option<Run>, JobQueueError> {
        self.0
            .latest_run(repository, principal, draft, instance)
            .await
    }

    async fn claim_run(
        &self,
        registrations: &[Registration],
        now: i64,
        lease: i64,
    ) -> Result<Option<Run>, JobQueueError> {
        self.0.claim_run(registrations, now, lease).await
    }

    async fn heartbeat_run(
        &self,
        run: RunId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Heartbeat, JobQueueError> {
        self.0.heartbeat_run(run, runners, now, lease).await
    }

    async fn held_run(&self, run: RunId, runners: &[RegistrationId]) -> Result<Run, JobQueueError> {
        self.0.held_run(run, runners).await
    }

    async fn begin_report(
        &self,
        run: RunId,
        runners: &[RegistrationId],
        now: i64,
        lease: i64,
    ) -> Result<Run, JobQueueError> {
        self.0.begin_report(run, runners, now, lease).await
    }

    async fn complete_run(
        &self,
        run: RunId,
        runners: &[RegistrationId],
        result: RunResult,
    ) -> Result<Run, JobQueueError> {
        self.0.complete_run(run, runners, result).await
    }

    async fn expire_runs(&self, now: i64) -> Result<(), JobQueueError> {
        self.0.expire_runs(now).await
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
    let log = Arc::new(FailsMainSwap(harness.log.clone()));
    let queue = Arc::new(RecordRefusingQueue(harness.queue.clone()));
    let checks = Arc::new(PostLandChecks::new(
        PostLandChecksAdapters {
            objects: harness.objects.clone(),
            derived: harness.derived.clone(),
            log: log.clone(),
            directory: harness.directory.clone(),
            queue: queue.clone(),
            definitions: harness.definitions.clone(),
        },
        LandingAttempts::default(),
    ));
    let service = JobsService::new(
        JobsAdapters {
            registry: harness.registry.clone(),
            queue,
            cache_refs: harness.cache_refs.clone(),
            objects: harness.objects.clone(),
            derived: harness.derived.clone(),
            log,
            drafts: harness.drafts.clone(),
            authorisation: harness.authorisation.clone(),
            directory: harness.directory.clone(),
        },
        checks,
        30,
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
