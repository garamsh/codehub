//! A revert that loses the compare-and-swap on `main` is tried again, a bounded number of times.

#[cfg(test)]
pub mod common;

use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, Ordering};

use adapter_memory::MemoryChangesetLog;
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::job::{JobResult, Outcome};
use app::domain::repository::RepositoryId;
use app::domain::revert::Revert;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driving::jobs::{Jobs, JobsError};
use app::port::driving::revert::RevertError;
use app::port::driving::source_control::LandError;
use app::use_case::jobs::{JobsAdapters, JobsService};
use app::use_case::landing_attempts::LandingAttempts;
use common::Harness;
use test_support::contract::runner_deployment::{
    RunnerDeployment, check, definitions, land, organisation, runner, text,
};
use test_support::contract::{Deployment, Repo};

/// A log that loses the compare-and-swap on `main` the first `losses` times it is asked, as if
/// another landing had moved `main` first, and counts the asks.
struct LosingLog<'a> {
    inner: &'a MemoryChangesetLog,
    losses: AtomicU32,
    asked: AtomicU32,
}

impl ChangesetLog for LosingLog<'_> {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        self.inner.append(repository, changeset).await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        self.inner.get(repository, id).await
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        self.inner.read_ref(repository, name).await
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        if name == Ref::Main {
            self.asked.fetch_add(1, Ordering::SeqCst);
            let lose = self
                .losses
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                    left.checked_sub(1)
                })
                .is_ok();
            if lose {
                return Err(ChangesetLogError::RefMoved { current: expected });
            }
        }
        self.inner
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

fn failed() -> JobResult {
    JobResult {
        outcome: Outcome::Failed,
        exit_code: Some(1),
        output: Vec::new(),
    }
}

/// A repository whose latest changeset has a blocking check that a runner holds, and the log that
/// loses `losses` compare-and-swaps.
#[cfg(test)]
async fn setup(
    harness: &Harness,
    losses: u32,
) -> (
    Repo<'_, Harness>,
    Address,
    app::domain::job::JobId,
    LosingLog<'_>,
) {
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
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job.id;
    let log = LosingLog {
        inner: &harness.log,
        losses: AtomicU32::new(losses),
        asked: AtomicU32::new(0),
    };
    (repo, bad, job, log)
}

#[cfg(test)]
async fn report(
    harness: &Harness,
    log: &LosingLog<'_>,
    job: app::domain::job::JobId,
    attempts: u32,
) -> Result<(), JobsError> {
    let service = JobsService::new(
        JobsAdapters {
            registry: &harness.registry,
            queue: &harness.queue,
            objects: &harness.objects,
            derived: &harness.derived,
            log,
            directory: &harness.directory,
            definitions: &harness.definitions,
        },
        30,
        LandingAttempts::new(NonZeroU32::new(attempts).unwrap()),
    );
    let bot = harness.caller("bot").await;
    service.report(&bot, job, failed(), 5).await
}

#[tokio::test]
async fn a_revert_that_lost_the_compare_and_swap_is_tried_again() {
    let harness = Harness::start().await;
    let (repo, bad, job, log) = setup(&harness, 2).await;

    report(&harness, &log, job, 5).await.unwrap();

    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    assert_eq!(log.asked.load(Ordering::SeqCst), 3);
    assert_eq!(repo.read("a").await.as_deref(), Some("1\n"));
    let ada = harness.caller("ada").await;
    let jobs =
        app::port::driving::checks::Checks::jobs_of(&harness.checks(), &ada, repo.id, bad, 0)
            .await
            .unwrap();
    assert_eq!(jobs[0].revert, Some(Revert::Landed(reverted)));
}

#[tokio::test]
async fn a_revert_that_keeps_losing_the_compare_and_swap_is_given_up_after_its_attempts() {
    let harness = Harness::start().await;
    let (repo, bad, job, log) = setup(&harness, u32::MAX).await;

    let error = report(&harness, &log, job, 3).await.unwrap_err();

    assert!(
        matches!(
            error,
            JobsError::Revert(RevertError::Land(LandError::MainMoved { .. }))
        ),
        "{error:?}"
    );
    assert_eq!(log.asked.load(Ordering::SeqCst), 3);
    assert_eq!(repo.main().await, Some(bad));
}
