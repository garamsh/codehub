//! Checks queued on landing, run by runners, and what they do to `last-green`.

use crate::contract::runner_deployment::{
    RunnerDeployment, check, land, land_checks, organisation, runner,
};
use app::domain::address::Address;
use app::domain::job::{Heartbeat, Job, JobResult, JobState, Outcome, StopReason};
use app::domain::path::Prefix;
use app::domain::principal::Caller;
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::job_queue::JobQueueError;
use app::port::driven::membership::{Access, Role};
use app::port::driving::checks::{Checks, ChecksError};
use app::port::driving::jobs::Jobs;
use app::port::driving::members::Members;
use app::use_case::administration::Administration;
use app::use_case::members::{MembersAdapters, MembersService};

fn passed() -> JobResult {
    JobResult {
        outcome: Outcome::Passed,
        exit_code: 0,
        output: b"ok".to_vec(),
    }
}

fn failed() -> JobResult {
    JobResult {
        outcome: Outcome::Failed,
        exit_code: 1,
        output: b"broken".to_vec(),
    }
}

async fn last_green<H: RunnerDeployment>(
    harness: &H,
    caller: &Caller,
    repo: &crate::contract::Repo<'_, H>,
) -> Option<Address> {
    harness.checks().last_green(caller, repo.id).await.unwrap()
}

/// Claims each queued job in turn as `bot` at time `now` and reports each by its check name:
/// `failing` ones fail, the others pass.
async fn run_all<H: RunnerDeployment>(harness: &H, bot: &Caller, failing: &[&str], now: i64) {
    while let Some(claimed) = harness.jobs().claim(bot, now).await.unwrap() {
        let result = if failing.contains(&claimed.job.check.name.as_str()) {
            failed()
        } else {
            passed()
        };
        harness
            .jobs()
            .report(bot, claimed.job.id, result, now)
            .await
            .unwrap();
    }
}

async fn jobs<H: RunnerDeployment>(
    harness: &H,
    caller: &Caller,
    repo: &crate::contract::Repo<'_, H>,
    changeset: Address,
    now: i64,
) -> Vec<Job> {
    harness
        .checks()
        .jobs_of(caller, repo.id, changeset, now)
        .await
        .unwrap()
}

/// Landing a changeset that defines checks queues one job per check.
pub async fn landing_queues_one_job_per_defined_check<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(
        &repo,
        &ada,
        "checks",
        &[check("unit"), check("lint").non_blocking()],
    )
    .await;

    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    let mut names: Vec<&str> = queued.iter().map(|j| j.check.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["lint", "unit"]);
    assert!(queued.iter().all(|j| j.state == JobState::Queued));
    assert!(queued.iter().all(|j| j.changeset == landed));
    assert_eq!(
        queued
            .iter()
            .find(|j| j.check.name.as_str() == "lint")
            .map(|j| j.check.blocking),
        Some(false)
    );
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
}

/// A changeset landed where the checks file is absent queues nothing and becomes `last-green`.
pub async fn a_landing_without_a_checks_file_queues_nothing_and_is_last_green<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land(&repo, &ada, "plain", &[("a", "a\n")]).await.unwrap();

    assert!(jobs(&harness, &ada, &repo, landed, 0).await.is_empty());
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(landed));
}

/// A changeset whose checks file defines only non-blocking checks becomes `last-green` at once.
pub async fn a_landing_with_only_non_blocking_checks_is_last_green<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(&repo, &ada, "checks", &[check("lint").non_blocking()]).await;

    assert_eq!(jobs(&harness, &ada, &repo, landed, 0).await.len(), 1);
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(landed));
}

/// A checks file that cannot be read becomes one failed blocking job, and the changeset is not
/// `last-green`.
pub async fn an_unreadable_checks_file_gives_one_failed_blocking_job<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let file = harness.definitions().file().to_string();
    let text = "[[check]\nthis is not a check file\n";
    harness.teach(
        text,
        Err(CheckFileError::Syntax {
            line: 1,
            description: "not a check file".to_owned(),
        }),
    );
    let landed = land(&repo, &ada, "broken", &[(&file, text)]).await.unwrap();

    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].state, JobState::Failed);
    assert!(queued[0].check.blocking);
    assert_eq!(queued[0].check.name.as_str(), "checks-file");
    assert!(!queued[0].output.is_empty());
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
}

/// `last-green` moves to a changeset once every blocking check of it has passed, and not before.
pub async fn last_green_moves_once_every_blocking_check_passed<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(&repo, &ada, "checks", &[check("unit"), check("lint")]).await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;

    let first = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    harness
        .jobs()
        .report(&bot, first.job.id, passed(), 1)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, None);

    let second = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    harness
        .jobs()
        .report(&bot, second.job.id, passed(), 3)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(landed));

    let done = jobs(&harness, &ada, &repo, landed, 4).await;
    assert!(done.iter().all(|j| j.state == JobState::Passed));
    assert!(done.iter().all(|j| j.exit_code == Some(0)));
    assert!(done.iter().all(|j| j.output == b"ok"));
}

/// A failed blocking check keeps the changeset from `last-green`, and a failed non-blocking check
/// does not.
pub async fn only_a_failed_blocking_check_keeps_a_changeset_from_last_green<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;

    let soft = land_checks(
        &repo,
        &ada,
        "soft",
        &[check("unit"), check("lint").non_blocking()],
    )
    .await;
    run_all(&harness, &bot, &["lint"], 0).await;
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(soft));

    land_checks(&repo, &ada, "hard", &[check("unit2"), check("extra")]).await;
    run_all(&harness, &bot, &["unit2"], 10).await;
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(soft));
}

/// `last-green` never moves backward: a passing older changeset does not displace a newer one.
pub async fn last_green_never_moves_backward<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let older = land_checks(&repo, &ada, "older", &[check("one")]).await;
    let newer = land_checks(&repo, &ada, "newer", &[check("two")]).await;
    assert_ne!(older, newer);

    let first = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    let second = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    assert_eq!(first.job.changeset, older);
    assert_eq!(second.job.changeset, newer);

    harness
        .jobs()
        .report(&bot, second.job.id, passed(), 1)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(newer));
    harness
        .jobs()
        .report(&bot, first.job.id, passed(), 2)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(newer));
    assert_eq!(
        harness
            .log()
            .read_ref(repo.id, Ref::LastGreen)
            .await
            .unwrap(),
        Some(newer)
    );
}

/// A heartbeat keeps a job leased past the lease of the claim.
pub async fn a_heartbeat_extends_the_lease<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let one = runner(&harness, &ada, &organisation("acme"), "one", &[]).await;
    let two = runner(&harness, &ada, &organisation("acme"), "two", &[]).await;

    let job = harness.jobs().claim(&one, 0).await.unwrap().unwrap().job.id;
    assert_eq!(
        harness.jobs().heartbeat(&one, job, 20).await.unwrap(),
        Heartbeat::Continue
    );
    // 40 is past the claim's lease of 30 but inside the heartbeat's.
    assert!(harness.jobs().claim(&two, 40).await.unwrap().is_none());
    let held = jobs(&harness, &ada, &repo, landed, 40).await;
    assert!(matches!(held[0].state, JobState::Running { .. }));
}

/// A job whose lease lapses without a heartbeat is queued again for another runner.
pub async fn a_job_whose_lease_lapses_is_queued_again<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let one = runner(&harness, &ada, &organisation("acme"), "one", &[]).await;
    let two = runner(&harness, &ada, &organisation("acme"), "two", &[]).await;

    let job = harness.jobs().claim(&one, 0).await.unwrap().unwrap().job.id;
    assert!(harness.jobs().claim(&two, 29).await.unwrap().is_none());
    let again = harness.jobs().claim(&two, 30).await.unwrap().unwrap();
    assert_eq!(again.job.id, job);
    // The first runner no longer holds it.
    assert!(
        harness
            .jobs()
            .report(&one, job, passed(), 31)
            .await
            .is_err()
    );
    harness
        .jobs()
        .report(&two, job, passed(), 31)
        .await
        .unwrap();
}

/// A running job that passes its deadline is timed out, tells its runner to stop, and keeps the
/// changeset from `last-green`.
pub async fn a_job_past_its_deadline_times_out<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(
        &repo,
        &ada,
        "checks",
        &[check("slow").timing_out_after(100)],
    )
    .await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;

    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job.id;
    for now in [20, 40, 60, 80] {
        assert_eq!(
            harness.jobs().heartbeat(&bot, job, now).await.unwrap(),
            Heartbeat::Continue
        );
    }
    assert_eq!(
        harness.jobs().heartbeat(&bot, job, 100).await.unwrap(),
        Heartbeat::Stop(StopReason::TimedOut)
    );
    let held = jobs(&harness, &ada, &repo, landed, 100).await;
    assert_eq!(held[0].state, JobState::TimedOut);
    assert!(
        harness
            .jobs()
            .report(&bot, job, passed(), 101)
            .await
            .is_err()
    );
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
}

/// Cancelling a job tells its runner to stop and keeps the changeset from `last-green`.
pub async fn a_cancelled_job_tells_its_runner_to_stop<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job.id;

    let cancelled = harness.checks().cancel(&ada, repo.id, job).await.unwrap();
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert_eq!(
        harness.jobs().heartbeat(&bot, job, 1).await.unwrap(),
        Heartbeat::Stop(StopReason::Cancelled)
    );
    assert!(harness.jobs().report(&bot, job, passed(), 2).await.is_err());
    assert_eq!(
        jobs(&harness, &ada, &repo, landed, 3).await[0].state,
        JobState::Cancelled
    );
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
    assert_eq!(
        harness
            .checks()
            .cancel(&ada, repo.id, job)
            .await
            .unwrap_err(),
        ChecksError::Queue(JobQueueError::Finished(job))
    );
}

/// Only a caller who reads the repository sees its jobs and `last-green`, and only an
/// administrator of its organisation cancels a job.
pub async fn checks_are_visible_to_readers_and_cancelled_by_administrators<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let job = jobs(&harness, &ada, &repo, landed, 0).await[0].id;
    let eve = harness.caller("eve").await;
    let rae = harness.caller("rae").await;
    let members = MembersService::new(
        MembersAdapters {
            directory: harness.directory(),
            authorisation: harness.authorisation(),
            membership: harness.membership(),
        },
        Administration::BuiltIn,
    );
    let acme = organisation("acme");
    members
        .add_member(&ada, &acme, rae.principal(), Role::Member)
        .await
        .unwrap();
    members
        .add_grant(
            &ada,
            &acme,
            rae.principal(),
            &"app".parse().unwrap(),
            Prefix::whole(),
            Access::Read,
        )
        .await
        .unwrap();

    // Someone who cannot read the repository sees nothing of it.
    let seen = harness.checks().jobs_of(&eve, repo.id, landed, 0).await;
    assert!(matches!(seen, Err(ChecksError::Access(_))), "{seen:?}");
    let green = harness.checks().last_green(&eve, repo.id).await;
    assert!(matches!(green, Err(ChecksError::Access(_))), "{green:?}");

    // A reader sees the jobs but may not cancel one.
    assert_eq!(jobs(&harness, &rae, &repo, landed, 0).await.len(), 1);
    assert_eq!(last_green(&harness, &rae, &repo).await, None);
    let cancel = harness.checks().cancel(&rae, repo.id, job).await;
    assert!(matches!(cancel, Err(ChecksError::Access(_))), "{cancel:?}");
    assert_eq!(
        jobs(&harness, &ada, &repo, landed, 0).await[0].state,
        JobState::Queued
    );

    // An administrator cancels it.
    harness.checks().cancel(&ada, repo.id, job).await.unwrap();
}
