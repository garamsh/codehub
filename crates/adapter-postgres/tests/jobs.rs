//! What only a shared database shows about the job queue: concurrent claimers, and a claim and a
//! heartbeat that leave a lapsed lease or a passed deadline to expire.

#[cfg(test)]
pub mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use app::domain::changeset::Changeset;
use app::domain::check::{CheckDefinition, CheckKey, Command, Timeout};
use app::domain::job::{Heartbeat, Job, JobState, NewJob, Resolution, StopReason};
use app::domain::name::Label;
use app::domain::runner::{NewRegistration, Registration};
use app::port::driven::changeset_log::ChangesetLog;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driven::runner_registry::RunnerRegistry;
use common::Harness;
use test_support::contract::Deployment;
use tokio::task::JoinSet;

#[cfg(test)]
const JOBS: usize = 100;

#[cfg(test)]
const CLAIMERS: usize = 32;

/// One repository of one organisation, a runner registered for all of it, and `count` queued jobs
/// of one changeset, each with a check of `timeout` seconds. The jobs are read back and asserted
/// queued.
#[cfg(test)]
async fn queued(harness: &Harness, count: usize, timeout: u64) -> (Registration, Vec<Job>) {
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let changeset = harness
        .log
        .append(
            repo.id,
            Changeset {
                parents: Vec::new(),
                author: "ada".to_owned(),
                timestamp: 0,
                message: "checks".to_owned(),
                changes: std::collections::BTreeMap::new(),
            },
        )
        .await
        .unwrap();
    let registered = NewRegistration {
        organisation: "acme".parse().unwrap(),
        repository: None,
        principal: Harness::principal("bot"),
        labels: BTreeSet::<Label>::new(),
    };
    let id = harness.registry.register(registered.clone()).await.unwrap();
    let new: Vec<NewJob> = (0..count)
        .map(|n| NewJob {
            organisation: "acme".parse().unwrap(),
            repository: repo.id,
            changeset,
            check: CheckDefinition::new(
                format!("check-{n}").parse().unwrap(),
                Command::new("run").unwrap(),
                BTreeSet::new(),
                true,
                Timeout::new(timeout).unwrap(),
            ),
        })
        .collect();
    harness.queue.queue(new).await.unwrap();
    // A queued job waits until it is resolved; these have no needs, so each is queued as it is.
    let waiting = harness
        .queue
        .of_changeset(repo.id, changeset)
        .await
        .unwrap();
    for job in &waiting {
        assert_eq!(job.state, JobState::Waiting);
        let key = CheckKey::new(&job.check, &[], &[]);
        assert!(
            harness
                .queue
                .resolve(job.id, Resolution::Queue { key })
                .await
                .unwrap()
        );
    }
    let held = harness
        .queue
        .of_changeset(repo.id, changeset)
        .await
        .unwrap();
    assert_eq!(held.len(), count);
    assert!(held.iter().all(|job| job.state == JobState::Queued));
    (Registration { id, registered }, held)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claimers_are_handed_each_job_exactly_once() {
    let harness = Arc::new(Harness::with_pool(16).await);
    let (registration, held) = queued(&harness, JOBS, 600).await;
    let registrations = Arc::new(vec![registration]);

    let mut claimers = JoinSet::new();
    for _ in 0..CLAIMERS {
        let harness = Arc::clone(&harness);
        let registrations = Arc::clone(&registrations);
        claimers.spawn(async move {
            let mut claimed = Vec::new();
            while let Some(job) = harness.queue.claim(&registrations, 0, 30).await.unwrap() {
                claimed.push(job.id);
            }
            claimed
        });
    }
    let mut handed_out = Vec::new();
    while let Some(claimer) = claimers.join_next().await {
        handed_out.extend(claimer.unwrap());
    }

    let distinct: BTreeSet<_> = handed_out.iter().copied().collect();
    assert_eq!(
        handed_out.len(),
        JOBS,
        "a job was handed out twice or never"
    );
    assert_eq!(distinct, held.iter().map(|job| job.id).collect());
}

#[tokio::test]
async fn a_claim_leaves_a_job_whose_lease_lapsed_to_the_expiry() {
    let harness = Harness::new().await;
    let (registration, _) = queued(&harness, 1, 600).await;
    let registrations = [registration];

    let first = harness.queue.claim(&registrations, 0, 30).await.unwrap();
    let first = first.unwrap();
    assert!(
        harness
            .queue
            .claim(&registrations, 30, 30)
            .await
            .unwrap()
            .is_none()
    );
    assert!(harness.queue.expire(30).await.unwrap().is_empty());
    let again = harness
        .queue
        .claim(&registrations, 30, 30)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(again.id, first.id);
    assert_eq!(
        again.state,
        JobState::Running {
            lease_expires_at: 60,
            deadline: 630,
        }
    );
}

#[tokio::test]
async fn a_heartbeat_past_the_deadline_tells_the_runner_to_stop_without_a_separate_expiry() {
    let harness = Harness::new().await;
    let (registration, _) = queued(&harness, 1, 100).await;
    let registrations = [registration.clone()];
    let job = harness
        .queue
        .claim(&registrations, 0, 30)
        .await
        .unwrap()
        .unwrap();

    for now in [20, 40, 60, 80] {
        assert_eq!(
            harness
                .queue
                .heartbeat(job.id, &[registration.id], now, 30)
                .await,
            Ok(Heartbeat::Continue)
        );
    }
    assert_eq!(
        harness
            .queue
            .heartbeat(job.id, &[registration.id], 100, 30)
            .await,
        Ok(Heartbeat::Stop(StopReason::TimedOut))
    );
}

#[tokio::test]
async fn a_heartbeat_after_the_expiry_returned_the_job_to_the_queue_finds_it_queued() {
    let harness = Harness::new().await;
    let (registration, _) = queued(&harness, 1, 600).await;
    let registrations = [registration.clone()];
    let job = harness
        .queue
        .claim(&registrations, 0, 30)
        .await
        .unwrap()
        .unwrap();
    harness.queue.expire(30).await.unwrap();

    assert_eq!(
        harness
            .queue
            .heartbeat(job.id, &[registration.id], 30, 30)
            .await,
        Err(JobQueueError::NotFound(job.id))
    );
}
