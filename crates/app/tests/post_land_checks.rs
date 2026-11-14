//! The post-land checks (ADR 0035) through their interface: queuing after a landing, expiry and
//! settling an ended job.

#[cfg(test)]
pub mod common;

use adapter_memory::MemoryJobQueue;
use app::domain::address::Address;
use app::domain::job::{Heartbeat, Job, JobId, JobResult, NewJob, Outcome};
use app::domain::name::OrganisationName;
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::domain::runner::{Registration, RegistrationId};
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driven::runner_registry::RunnerRegistry;
use app::port::driving::jobs::Jobs;
use app::port::driving::revert::{QueueChecksError, RevertError};
use app::port::driving::source_control::{LandError, Landing, SourceControl, SourceControlError};
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::post_land_checks::{
    AfterLandingError, PostLandChecks, PostLandChecksAdapters, SettleError,
};
use app::use_case::source_control::AfterLanding;
use common::Harness;
use test_support::contract::runner_deployment::{
    LEASE, RunnerDeployment, check, definitions, land, organisation, runner, text,
};
use test_support::contract::{Deployment, draft};

const QUEUE_DOWN: &str = "queue is down";

/// A queue that refuses to queue and does everything else as the [`MemoryJobQueue`] it wraps does.
struct RefusingQueue<'a>(&'a MemoryJobQueue);

impl JobQueue for RefusingQueue<'_> {
    async fn queue(&self, _jobs: Vec<NewJob>) -> Result<Vec<JobId>, JobQueueError> {
        Err(JobQueueError::Backend(QUEUE_DOWN.to_owned()))
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

    async fn reserve_revert(&self, job: JobId) -> Result<bool, JobQueueError> {
        self.0.reserve_revert(job).await
    }

    async fn record_revert(&self, job: JobId, revert: Revert) -> Result<Job, JobQueueError> {
        self.0.record_revert(job, revert).await
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

#[cfg(test)]
fn adapters<'a, Q>(
    harness: &'a Harness,
    queue: &'a Q,
) -> PostLandChecksAdapters<
    'a,
    adapter_memory::MemoryObjectStore,
    adapter_memory::MemoryDerivedData,
    adapter_memory::MemoryChangesetLog,
    adapter_memory::MemoryRepositoryDirectory,
    Q,
    adapter_memory::MemoryCheckDefinitions,
> {
    PostLandChecksAdapters {
        objects: harness.objects(),
        derived: harness.derived(),
        log: harness.log(),
        directory: harness.directory(),
        queue,
        definitions: harness.definitions(),
    }
}

#[cfg(test)]
fn result(outcome: Outcome) -> JobResult {
    JobResult {
        outcome,
        exit_code: Some(i32::from(outcome != Outcome::Passed)),
        output: Vec::new(),
    }
}

/// A repository with `a` changed by a landing that defines one blocking check, the job of which a
/// runner has claimed at time 0, and what is needed to end it.
struct HeldJob<'a> {
    repo: test_support::contract::Repo<'a, Harness>,
    bad: Address,
    job: Job,
    runners: Vec<RegistrationId>,
}

#[cfg(test)]
async fn claimed(harness: &Harness) -> HeldJob<'_> {
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", "1\n")]).await.unwrap();
    let specs = [check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    let bad = land(&repo, &ada, "bad", &[(&file, &body), ("a", "2\n")])
        .await
        .unwrap();
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job;
    let runners = harness
        .registry
        .of_principal(bot.principal())
        .await
        .unwrap()
        .iter()
        .map(|registration| registration.id)
        .collect();
    HeldJob {
        repo,
        bad,
        job,
        runners,
    }
}

#[tokio::test]
async fn a_landing_whose_checks_cannot_be_queued_names_the_queue() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let specs = [check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    let name = draft("checks");
    repo.stage(&ada, &name, &[(&file, &body)]).await;
    let landing = Landing {
        message: "checks".to_owned(),
        timestamp: 0,
    };
    let landed = harness
        .source_control()
        .land(&ada, repo.id, &name, landing)
        .await
        .unwrap();
    let refusing = RefusingQueue(&harness.queue);
    let checks = PostLandChecks::new(adapters(&harness, &refusing), LandingAttempts::default());

    let error = checks.after_landing(repo.id, landed, 0).await.unwrap_err();

    assert_eq!(
        error,
        AfterLandingError::Queue(QueueChecksError::Queue(JobQueueError::Backend(
            QUEUE_DOWN.to_owned()
        )))
    );
    assert!(
        harness
            .queue
            .of_changeset(repo.id, landed)
            .await
            .unwrap()
            .is_empty()
    );
    let checks = PostLandChecks::new(
        adapters(&harness, &harness.queue),
        LandingAttempts::default(),
    );
    checks.after_landing(repo.id, landed, 0).await.unwrap();
    let queued = harness.queue.of_changeset(repo.id, landed).await.unwrap();
    assert_eq!(queued.len(), 1);
}

#[tokio::test]
async fn a_landing_whose_checks_cannot_be_queued_still_landed_and_says_so() {
    let harness = Harness::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let specs = [check("unit")];
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    let file = harness.definitions().file().to_string();
    let name = draft("checks");
    repo.stage(&ada, &name, &[(&file, &body)]).await;
    let refusing = RefusingQueue(&harness.queue);
    let checked = harness
        .source_control()
        .with_after_landing(PostLandChecks::new(
            adapters(&harness, &refusing),
            LandingAttempts::default(),
        ));

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
    assert!(cause.contains(QUEUE_DOWN), "{cause}");
    let message = error.to_string();
    assert!(message.contains(&landed.to_string()), "{message}");
    assert!(message.contains("landed"), "{message}");
    assert!(message.contains("not queued"), "{message}");
}

#[tokio::test]
async fn settling_a_failed_blocking_job_reverts_its_changeset() {
    let harness = Harness::start().await;
    let held = claimed(&harness).await;
    let ended = harness
        .queue
        .complete(held.job.id, &held.runners, result(Outcome::Failed))
        .await
        .unwrap();
    let checks = PostLandChecks::new(
        adapters(&harness, &harness.queue),
        LandingAttempts::default(),
    );

    checks.settle(&ended, 5).await.unwrap();

    let reverted = held.repo.main().await.unwrap();
    assert_ne!(reverted, held.bad);
    assert_eq!(held.repo.read("a").await.as_deref(), Some("1\n"));
    let recorded = harness
        .queue
        .of_changeset(held.repo.id, held.bad)
        .await
        .unwrap();
    assert_eq!(recorded[0].revert, Some(Revert::Landed(reverted)));
}

#[tokio::test]
async fn settling_the_last_passing_blocking_job_moves_last_green() {
    let harness = Harness::start().await;
    let held = claimed(&harness).await;
    let before = harness
        .log
        .read_ref(held.repo.id, Ref::LastGreen)
        .await
        .unwrap();
    assert_ne!(before, Some(held.bad));
    let ended = harness
        .queue
        .complete(held.job.id, &held.runners, result(Outcome::Passed))
        .await
        .unwrap();
    let checks = PostLandChecks::new(
        adapters(&harness, &harness.queue),
        LandingAttempts::default(),
    );

    checks.settle(&ended, 5).await.unwrap();

    let after = harness
        .log
        .read_ref(held.repo.id, Ref::LastGreen)
        .await
        .unwrap();
    assert_eq!(after, Some(held.bad));
    assert_eq!(held.repo.main().await, Some(held.bad));
}

#[tokio::test]
async fn expiring_a_blocking_job_past_its_timeout_reverts_its_changeset() {
    let harness = Harness::start().await;
    let held = claimed(&harness).await;
    let checks = PostLandChecks::new(
        adapters(&harness, &harness.queue),
        LandingAttempts::default(),
    );

    let timeout = i64::try_from(held.job.check.timeout.seconds()).unwrap();

    checks.expire(LEASE - 1).await.unwrap();
    assert_eq!(held.repo.main().await, Some(held.bad));

    checks.expire(timeout).await.unwrap();
    let reverted = held.repo.main().await.unwrap();
    assert_ne!(reverted, held.bad);
    assert_eq!(held.repo.read("a").await.as_deref(), Some("1\n"));
}

#[tokio::test]
async fn a_revert_whose_checks_cannot_be_queued_has_landed_and_names_the_queue() {
    let harness = Harness::start().await;
    let held = claimed(&harness).await;
    let ended = harness
        .queue
        .complete(held.job.id, &held.runners, result(Outcome::Failed))
        .await
        .unwrap();
    let refusing = RefusingQueue(&harness.queue);
    let checks = PostLandChecks::new(adapters(&harness, &refusing), LandingAttempts::default());

    let error = checks.settle(&ended, 5).await.unwrap_err();

    assert_eq!(
        error,
        SettleError::Revert(RevertError::ChecksNotQueued(QueueChecksError::Queue(
            JobQueueError::Backend(QUEUE_DOWN.to_owned())
        )))
    );
    assert_ne!(held.repo.main().await, Some(held.bad));
}
