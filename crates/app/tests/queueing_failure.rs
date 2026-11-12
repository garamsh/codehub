//! A landing whose checks cannot be queued still landed, and says so.

#[cfg(test)]
pub mod common;

use adapter_memory::MemoryJobQueue;
use app::domain::address::Address;
use app::domain::job::{Heartbeat, Job, JobId, JobResult, NewJob};
use app::domain::name::OrganisationName;
use app::domain::repository::RepositoryId;
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driving::source_control::{LandError, Landing, SourceControl, SourceControlError};
use app::use_case::checked_source_control::{CheckedAdapters, CheckedSourceControl};
use common::Harness;
use test_support::contract::runner_deployment::{RunnerDeployment, definitions, text};
use test_support::contract::{Deployment, draft};

/// A queue that refuses to queue and does everything else as [`MemoryJobQueue`] does.
#[derive(Default)]
struct RefusingQueue(MemoryJobQueue);

impl JobQueue for RefusingQueue {
    async fn queue(&self, _jobs: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError> {
        Err(JobQueueError::Backend("queue is down".to_owned()))
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

    async fn complete(
        &self,
        job: JobId,
        runners: &[RegistrationId],
        result: JobResult,
    ) -> Result<Job, JobQueueError> {
        self.0.complete(job, runners, result).await
    }

    async fn cancel(
        &self,
        organisation: &OrganisationName,
        job: JobId,
    ) -> Result<Job, JobQueueError> {
        self.0.cancel(organisation, job).await
    }

    async fn of_changeset(
        &self,
        repository: RepositoryId,
        changeset: Address,
    ) -> Result<Vec<Job>, JobQueueError> {
        self.0.of_changeset(repository, changeset).await
    }

    async fn expire(&self, now: i64) -> Result<Vec<Job>, JobQueueError> {
        self.0.expire(now).await
    }
}

#[tokio::test]
async fn a_landing_whose_checks_cannot_be_queued_names_the_landed_changeset() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let specs = [test_support::contract::runner_deployment::check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    let name = draft("checks");
    repo.stage(&ada, &name, &[(&file, &body)]).await;
    let queue = RefusingQueue::default();
    let checked = CheckedSourceControl::new(
        harness.source_control(),
        CheckedAdapters {
            objects: harness.objects(),
            derived: harness.derived(),
            log: harness.log(),
            directory: harness.directory(),
            queue: &queue,
            definitions: harness.definitions(),
        },
    );

    let landing = Landing {
        message: "checks".to_owned(),
        timestamp: 0,
    };
    let error = checked
        .land(&ada, repo.id, &name, landing)
        .await
        .unwrap_err();

    let landed = repo.main().await.expect("the changeset landed");
    let SourceControlError::Land(LandError::ChecksNotQueued {
        landed: named,
        cause,
    }) = &error
    else {
        panic!("expected ChecksNotQueued, got {error:?}");
    };
    assert_eq!(*named, landed);
    assert!(cause.contains("queue is down"), "{cause}");
    let message = error.to_string();
    assert!(message.contains(&landed.to_string()), "{message}");
    assert!(message.contains("landed"), "{message}");
    assert!(message.contains("not queued"), "{message}");
}
