//! A changeset whose blocking check failed or timed out is reverted by a new changeset on `main`.

use crate::contract::runner_deployment::{
    RunnerDeployment, Spec, check, land, land_checks, organisation, runner, text,
};
use crate::contract::{Repo, draft};
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::job::{Heartbeat, Job, JobResult, JobState, Outcome, StopReason};
use app::domain::principal::Caller;
use app::domain::revert::Revert;
use app::port::driven::changeset_log::ChangesetLog;
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};
use app::port::driving::checks::Checks;
use app::port::driving::jobs::Jobs;

/// Eight lines, so that a change to the first and one to the last do not overlap.
const BASE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n";

fn failed() -> JobResult {
    JobResult {
        outcome: Outcome::Failed,
        exit_code: Some(1),
        output: b"broken".to_vec(),
    }
}

fn passed() -> JobResult {
    JobResult {
        outcome: Outcome::Passed,
        exit_code: Some(0),
        output: b"ok".to_vec(),
    }
}

async fn changeset<H: RunnerDeployment>(repo: &Repo<'_, H>, id: Address) -> Changeset {
    repo.harness.log().get(repo.id, id).await.unwrap()
}

async fn jobs<H: RunnerDeployment>(repo: &Repo<'_, H>, ada: &Caller, landed: Address) -> Vec<Job> {
    repo.harness
        .checks()
        .jobs_of(ada, repo.id, landed, 0)
        .await
        .unwrap()
}

/// The job of `landed` for the check `name`.
async fn job_of<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    ada: &Caller,
    landed: Address,
    name: &str,
) -> Job {
    jobs(repo, ada, landed)
        .await
        .into_iter()
        .find(|job| job.check.name.as_str() == name)
        .unwrap()
}

/// Claims the next job as `bot` at `now` and reports it as `result`, returning it.
async fn report<H: RunnerDeployment>(
    harness: &H,
    bot: &Caller,
    result: JobResult,
    now: i64,
) -> Job {
    let claimed = harness.jobs().claim(bot, now).await.unwrap().unwrap().job;
    harness
        .jobs()
        .report(bot, claimed.id, result, now)
        .await
        .unwrap();
    claimed
}

/// Lands a changeset defining `specs` that also writes `files`.
async fn land_with_checks<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    ada: &Caller,
    name: &str,
    specs: &[Spec],
    files: &[(&str, &str)],
) -> Address {
    let body = text(specs);
    repo.harness.teach(
        &body,
        Ok(crate::contract::runner_deployment::definitions(specs)),
    );
    let file = repo.harness.definitions().file().to_string();
    let mut all = vec![(file.as_str(), body.as_str())];
    all.extend_from_slice(files);
    land(repo, ada, name, &all).await.unwrap()
}

/// Lands, without queuing its checks, a draft of `files`.
async fn land_plain<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    ada: &Caller,
    name: &str,
    files: &[(&str, &str)],
) -> Address {
    let name = draft(name);
    repo.stage(ada, &name, files).await;
    repo.land(ada, &name, "plain").await.unwrap()
}

/// A failing blocking check lands a changeset authored `codehub` that undoes its changeset, with
/// the check named in its message and the time of the report, and records it with the job.
pub async fn a_failing_blocking_check_reverts_its_changeset_as_a_new_changeset<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE), ("b", "b\n")])
        .await
        .unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n"), ("c", "new\n")],
    )
    .await;
    assert_eq!(repo.main().await, Some(bad));

    let ended = report(&harness, &bot, failed(), 7).await;

    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    let landed = changeset(&repo, reverted).await;
    assert_eq!(landed.parents, [bad]);
    assert_eq!(landed.author, "codehub");
    assert_eq!(landed.message, format!("Revert {bad}: check unit failed"));
    assert_eq!(landed.timestamp, 7);
    assert_eq!(repo.read("a").await.as_deref(), Some(BASE));
    assert_eq!(repo.read("b").await.as_deref(), Some("b\n"));
    assert_eq!(repo.read("c").await, None);
    assert_eq!(repo.read(".codehub/checks").await, None);
    let job = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(job.id, ended.id);
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.revert, Some(Revert::Landed(reverted)));
}

/// A blocking check that times out reverts its changeset too, and says so in the message.
pub async fn a_timed_out_blocking_check_reverts_its_changeset<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("slow").timing_out_after(100)],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;
    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job;
    assert_eq!(repo.main().await, Some(bad));

    // Reading the checks at the deadline times the job out, which reverts the changeset.
    let seen = harness
        .checks()
        .jobs_of(&ada, repo.id, bad, 150)
        .await
        .unwrap();

    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    let landed = changeset(&repo, reverted).await;
    assert_eq!(
        landed.message,
        format!("Revert {bad}: check slow timed out")
    );
    assert_eq!(landed.timestamp, 150);
    assert_eq!(repo.read("a").await.as_deref(), Some(BASE));
    assert_eq!(seen[0].id, claimed.id);
    let job = job_of(&repo, &ada, bad, "slow").await;
    assert_eq!(job.state, JobState::TimedOut);
    assert_eq!(job.revert, Some(Revert::Landed(reverted)));
}

/// A blocking job that times out when a runner's heartbeat arrives past its deadline still
/// reverts its changeset: the expiry the heartbeat triggers reaches the revert.
pub async fn a_blocking_check_timed_out_by_a_heartbeat_reverts_its_changeset<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("slow").timing_out_after(100)],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;
    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job;

    let answer = harness.jobs().heartbeat(&bot, claimed.id, 150).await;

    assert_eq!(answer, Ok(Heartbeat::Stop(StopReason::TimedOut)));
    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    assert_eq!(
        changeset(&repo, reverted).await.message,
        format!("Revert {bad}: check slow timed out")
    );
    let job = job_of(&repo, &ada, bad, "slow").await;
    assert_eq!(job.state, JobState::TimedOut);
    assert_eq!(job.revert, Some(Revert::Landed(reverted)));
}

/// A change that landed after the failed changeset survives its revert, whether it is to another
/// path or to other lines of the same file.
pub async fn a_later_non_overlapping_change_survives_the_revert<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE), ("b", "b\n")])
        .await
        .unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;
    let later = land_plain(
        &repo,
        &ada,
        "later",
        &[("a", "x\n2\n3\n4\n5\n6\n7\ny\n"), ("b", "later\n")],
    )
    .await;
    assert_eq!(repo.main().await, Some(later));

    report(&harness, &bot, failed(), 1).await;

    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, later);
    assert_eq!(changeset(&repo, reverted).await.parents, [later]);
    assert_eq!(
        repo.read("a").await.as_deref(),
        Some("1\n2\n3\n4\n5\n6\n7\ny\n")
    );
    assert_eq!(repo.read("b").await.as_deref(), Some("later\n"));
    let job = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(job.revert, Some(Revert::Landed(reverted)));
}

/// A revert that conflicts with a later change lands nothing, and the job records the paths that
/// conflict.
pub async fn a_conflicting_revert_lands_nothing_and_is_recorded<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE), ("b", "b\n")])
        .await
        .unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n"), ("b", "bad\n")],
    )
    .await;
    let later = land_plain(&repo, &ada, "later", &[("a", "y\n2\n3\n4\n5\n6\n7\n8\n")]).await;

    report(&harness, &bot, failed(), 1).await;

    assert_eq!(repo.main().await, Some(later));
    assert_eq!(
        repo.read("a").await.as_deref(),
        Some("y\n2\n3\n4\n5\n6\n7\n8\n")
    );
    let job = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(
        job.revert,
        Some(Revert::Conflicted(["a".parse().unwrap()].into()))
    );
}

/// A second blocking check of the same changeset that fails does not revert it again.
pub async fn a_second_failing_blocking_check_does_not_revert_twice<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit"), check("lint")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;

    report(&harness, &bot, failed(), 1).await;
    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    report(&harness, &bot, failed(), 2).await;

    assert_eq!(repo.main().await, Some(reverted));
    assert_eq!(changeset(&repo, reverted).await.parents, [bad]);
    let recorded: Vec<Option<Revert>> = jobs(&repo, &ada, bad)
        .await
        .into_iter()
        .map(|job| job.revert)
        .collect();
    assert_eq!(recorded.iter().filter(|revert| revert.is_some()).count(), 1);
    assert!(
        jobs(&repo, &ada, bad)
            .await
            .iter()
            .all(|job| job.state == JobState::Failed)
    );
}

/// A failing check that does not block reverts nothing.
pub async fn a_failing_non_blocking_check_reverts_nothing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("lint").non_blocking()],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;

    report(&harness, &bot, failed(), 1).await;

    assert_eq!(repo.main().await, Some(bad));
    let job = job_of(&repo, &ada, bad, "lint").await;
    assert_eq!(job.state, JobState::Failed);
    assert_eq!(job.revert, None);
}

/// A cancelled check reverts nothing.
pub async fn a_cancelled_check_reverts_nothing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;
    let job = job_of(&repo, &ada, bad, "unit").await;

    harness
        .checks()
        .cancel(&ada, repo.id, job.id)
        .await
        .unwrap();

    assert_eq!(repo.main().await, Some(bad));
    let cancelled = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(cancelled.state, JobState::Cancelled);
    assert_eq!(cancelled.revert, None);
}

/// A failing check of a revert the server made reverts nothing, so a revert is never reverted in
/// its turn.
pub async fn a_failing_check_of_a_revert_changeset_reverts_nothing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land_checks(&repo, &ada, "base", &[check("unit")]).await;
    report(&harness, &bot, passed(), 0).await;
    let bad = land_with_checks(&repo, &ada, "bad", &[check("unit")], &[("a", "changed\n")]).await;

    report(&harness, &bot, failed(), 1).await;
    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    // The revert restored the definitions, so its own check is queued.
    let queued = job_of(&repo, &ada, reverted, "unit").await;
    assert_eq!(queued.state, JobState::Queued);

    report(&harness, &bot, failed(), 2).await;

    assert_eq!(repo.main().await, Some(reverted));
    let ended = job_of(&repo, &ada, reverted, "unit").await;
    assert_eq!(ended.state, JobState::Failed);
    assert_eq!(ended.revert, None);
}

/// A landing whose check definitions cannot be read is reverted at once, as a failed blocking
/// check is.
pub async fn a_landing_with_unreadable_definitions_is_reverted<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let file = harness.definitions().file().to_string();
    let broken = "[[check]\nthis is not a check file\n";
    harness.teach(
        broken,
        Err(CheckFileError::Syntax {
            line: 1,
            description: "not a check file".to_owned(),
        }),
    );
    let bad = land(
        &repo,
        &ada,
        "broken",
        &[(&file, broken), ("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await
    .unwrap();

    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    let landed = changeset(&repo, reverted).await;
    assert_eq!(landed.parents, [bad]);
    assert_eq!(landed.author, "codehub");
    assert_eq!(
        landed.message,
        format!("Revert {bad}: check checks-file failed")
    );
    assert_eq!(repo.read("a").await.as_deref(), Some(BASE));
    assert_eq!(repo.read(&file).await, None);
    let job = job_of(&repo, &ada, bad, "checks-file").await;
    assert_eq!(job.revert, Some(Revert::Landed(reverted)));
}

/// A runner that reports it enforced the timeout, with no exit code, ends a blocking job timed out
/// and the changeset is reverted as for a server-side timeout.
pub async fn a_runner_reported_timeout_reverts_its_changeset<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;

    let result = JobResult {
        outcome: Outcome::TimedOut,
        exit_code: None,
        output: Vec::new(),
    };
    report(&harness, &bot, result, 5).await;

    let ended = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(ended.state, JobState::TimedOut);
    assert_eq!(ended.exit_code, None);
    let reverted = repo.main().await.unwrap();
    assert_ne!(reverted, bad);
    assert_eq!(
        changeset(&repo, reverted).await.message,
        format!("Revert {bad}: check unit timed out")
    );
    assert_eq!(ended.revert, Some(Revert::Landed(reverted)));
    assert_eq!(repo.read("a").await.as_deref(), Some(BASE));
}

/// A runner that reports the job cancelled ends it cancelled, and the changeset is not reverted.
pub async fn a_runner_reported_cancellation_reverts_nothing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    land(&repo, &ada, "base", &[("a", BASE)]).await.unwrap();
    let bad = land_with_checks(
        &repo,
        &ada,
        "bad",
        &[check("unit")],
        &[("a", "x\n2\n3\n4\n5\n6\n7\n8\n")],
    )
    .await;

    let result = JobResult {
        outcome: Outcome::Cancelled,
        exit_code: None,
        output: Vec::new(),
    };
    report(&harness, &bot, result, 5).await;

    assert_eq!(repo.main().await, Some(bad));
    let ended = job_of(&repo, &ada, bad, "unit").await;
    assert_eq!(ended.state, JobState::Cancelled);
    assert_eq!(ended.revert, None);
}
