//! Checks queued on landing, run by runners, and what they do to `last-green`.

use crate::contract::draft;
use crate::contract::runner_deployment::{
    LEASE, RunnerDeployment, Spec, check, land, land_checks, land_checks_in, land_checks_with,
    organisation, runner,
};
use app::domain::address::Address;
use app::domain::file::{File, FileKind};
use app::domain::job::{
    CacheRef, Heartbeat, Job, JobResult, JobState, Outcome, Produced, ProducedFile, StopReason,
    empty_output_tree,
};
use app::domain::path::{Component, Path, Prefix};
use app::domain::principal::Caller;
use app::port::driven::changeset_log::{ChangesetLog, Ref};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driven::membership::{Access, Role};
use app::port::driven::object_store::ObjectStore;
use app::port::driven::runner_registry::RunnerRegistry;
use app::port::driving::checks::{
    Checks, ChecksError, PlanError, PlanOutcome, PlannedCheck, RunReason,
};
use app::port::driving::jobs::{Claimed, Jobs, Upload};
use app::port::driving::members::Members;
use app::port::driving::source_control::LandError;
use app::use_case::administration::Administration;
use app::use_case::members::{MembersAdapters, MembersService};

fn passed() -> JobResult {
    JobResult {
        outcome: Outcome::Passed,
        exit_code: Some(0),
        output: b"ok".to_vec(),
    }
}

fn failed() -> JobResult {
    JobResult {
        outcome: Outcome::Failed,
        exit_code: Some(1),
        output: b"broken".to_vec(),
    }
}

async fn last_green<H: RunnerDeployment>(
    harness: &H,
    caller: &Caller,
    repo: &crate::contract::Repo<'_, H>,
) -> Option<Address> {
    harness
        .checks()
        .reference(caller, repo.id, Ref::LastGreen)
        .await
        .unwrap()
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
            .report(bot, claimed.job.id, result, Produced::default(), now)
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
    // The landing is reverted, and the revert, which defines no check, is `last-green` instead.
    assert_ne!(last_green(&harness, &ada, &repo).await, Some(landed));
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
        .report(&bot, first.job.id, passed(), Produced::default(), 1)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, None);

    let second = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    harness
        .jobs()
        .report(&bot, second.job.id, passed(), Produced::default(), 3)
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

    let hard = land_checks(&repo, &ada, "hard", &[check("unit2"), check("extra")]).await;
    run_all(&harness, &bot, &["unit2"], 10).await;
    // `hard` is reverted, and the revert's checks pass, so `last-green` is the revert.
    let green = last_green(&harness, &ada, &repo).await;
    assert_ne!(green, Some(hard));
    assert_eq!(green, repo.main().await);
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
        .report(&bot, second.job.id, passed(), Produced::default(), 1)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(newer));
    harness
        .jobs()
        .report(&bot, first.job.id, passed(), Produced::default(), 2)
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
            .report(&one, job, passed(), Produced::default(), 31)
            .await
            .is_err()
    );
    harness
        .jobs()
        .report(&two, job, passed(), Produced::default(), 31)
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
            .report(&bot, job, passed(), Produced::default(), 101)
            .await
            .is_err()
    );
    // The changeset is reverted, and the revert, which defines no check, is `last-green` instead.
    assert_ne!(last_green(&harness, &ada, &repo).await, Some(landed));
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
    assert!(
        harness
            .jobs()
            .report(&bot, job, passed(), Produced::default(), 2)
            .await
            .is_err()
    );
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
    for name in [Ref::Main, Ref::LastGreen] {
        let seen = harness.checks().reference(&eve, repo.id, name).await;
        assert!(matches!(seen, Err(ChecksError::Access(_))), "{seen:?}");
    }

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

/// The job queue answers a heartbeat past the job's deadline as timed out, and extends nothing,
/// before anything has expired the job.
pub async fn a_heartbeat_past_the_deadline_answers_timed_out<H: RunnerDeployment>() {
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
    let held: Vec<_> = harness
        .registry()
        .of_principal(bot.principal())
        .await
        .unwrap()
        .iter()
        .map(|registration| registration.id)
        .collect();

    assert_eq!(
        harness
            .queue()
            .heartbeat(job, &held, 99, LEASE)
            .await
            .unwrap(),
        Heartbeat::Continue
    );
    assert_eq!(
        harness
            .queue()
            .heartbeat(job, &held, 100, LEASE)
            .await
            .unwrap(),
        Heartbeat::Stop(StopReason::TimedOut)
    );
    // Nothing expired the job, so it still runs; reading it expires it.
    assert!(matches!(
        harness.queue().of_changeset(repo.id, landed).await.unwrap()[0].state,
        JobState::Running { .. }
    ));
}

/// The ref `main` is served as `last-green` is: the changeset it points at, `None` before the
/// first landing, and `last-green` stays where it is while `main` moves on.
pub async fn main_is_served_as_a_ref<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let before = harness
        .checks()
        .reference(&ada, repo.id, Ref::Main)
        .await
        .unwrap();
    assert_eq!(before, None);

    let first = land(&repo, &ada, "first", &[("a", "1\n")]).await.unwrap();
    land_checks(&repo, &ada, "hard", &[check("unit")]).await;
    let head = repo.main().await;

    let main = harness
        .checks()
        .reference(&ada, repo.id, Ref::Main)
        .await
        .unwrap();
    assert_eq!(main, head);
    assert_ne!(main, Some(first));
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(first));
}

/// The names the jobs of `changeset` show under, sorted.
async fn queued_ids<H: RunnerDeployment>(
    harness: &H,
    caller: &Caller,
    repo: &crate::contract::Repo<'_, H>,
    changeset: Address,
) -> Vec<String> {
    let mut ids: Vec<String> = jobs(harness, caller, repo, changeset, 0)
        .await
        .iter()
        .map(|job| job.check.id().to_string())
        .collect();
    ids.sort_unstable();
    ids
}

/// Checks files in several folders queue the checks of every one: a check of the root keeps its bare
/// name, and any other is named `<folder>:<name>`.
pub async fn checks_files_in_every_folder_queue_their_checks<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks_in(
        &repo,
        &ada,
        "folders",
        &[
            ("", &[check("unit")]),
            ("lib", &[check("build"), check("lint").non_blocking()]),
            ("svc/api", &[check("unit")]),
        ],
    )
    .await;

    assert_eq!(
        queued_ids(&harness, &ada, &repo, landed).await,
        ["lib:build", "lib:lint", "svc/api:unit", "unit"]
    );
    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    assert!(queued.iter().all(|job| job.state == JobState::Queued));
    let lib = queued
        .iter()
        .find(|job| job.check.id().to_string() == "lib:build")
        .unwrap();
    assert_eq!(
        lib.check.folder,
        Prefix::new(vec![Component::new(b"lib".to_vec()).unwrap()])
    );
    assert_eq!(lib.check.name.as_str(), "build");
}

/// A job carries the `needs` its check file gave, as written.
pub async fn a_queued_job_carries_the_needs_of_its_check<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks_in(
        &repo,
        &ada,
        "needs",
        &[
            ("", &[check("unit").needing(&["//lib:build"])]),
            ("lib", &[check("build"), check("test").needing(&["build"])]),
        ],
    )
    .await;

    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    let needs_of = |id: &str| -> Vec<String> {
        queued
            .iter()
            .find(|job| job.check.id().to_string() == id)
            .unwrap()
            .check
            .needs
            .iter()
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(needs_of("unit"), ["//lib:build"]);
    assert_eq!(needs_of("lib:test"), ["build"]);
    assert!(needs_of("lib:build").is_empty());
}

/// Lands the checks `folders`, which are fault free, then the checks `broken`, and expects the
/// second landing to be recorded as one failed blocking job named for the file `file_in` and the
/// first to queue its checks normally.
async fn a_needs_fault_is_one_failed_blocking_job<H: RunnerDeployment>(
    control: &[(&str, &[Spec])],
    broken: &[(&str, &[Spec])],
    control_ids: &[&str],
    file_in: &str,
) {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let file = harness.definitions().file().to_string();

    let fine = land_checks_in(&repo, &ada, "control", control).await;
    assert_eq!(queued_ids(&harness, &ada, &repo, fine).await, control_ids);
    let fine_jobs = jobs(&harness, &ada, &repo, fine, 0).await;
    assert!(
        fine_jobs
            .iter()
            .all(|job| matches!(job.state, JobState::Queued | JobState::Waiting))
    );

    let landed = land_checks_in(&repo, &ada, "broken", broken).await;
    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].state, JobState::Failed);
    assert!(queued[0].check.blocking);
    assert_eq!(queued[0].check.name.as_str(), "checks-file");
    let declaring = if file_in.is_empty() {
        file
    } else {
        format!("{file_in}/{file}")
    };
    assert_eq!(queued[0].check.command.as_str(), declaring);
    assert!(!queued[0].output.is_empty());
    assert_ne!(last_green(&harness, &ada, &repo).await, Some(landed));
}

/// A `needs` naming no check becomes one failed blocking job named for the file that declares the
/// check, like a file that does not parse.
pub async fn a_need_naming_no_check_gives_one_failed_blocking_job<H: RunnerDeployment>() {
    a_needs_fault_is_one_failed_blocking_job::<H>(
        &[
            ("", &[check("unit")]),
            ("lib", &[check("build").needing(&["//:unit"])]),
        ],
        &[
            ("", &[check("unit")]),
            ("lib", &[check("build").needing(&["//:missing"])]),
        ],
        &["lib:build", "unit"],
        "lib",
    )
    .await;
}

/// A cycle of `needs` becomes one failed blocking job named for the file that declares a check on
/// the cycle.
pub async fn a_cycle_of_needs_gives_one_failed_blocking_job<H: RunnerDeployment>() {
    a_needs_fault_is_one_failed_blocking_job::<H>(
        &[("", &[check("a").needing(&["b"]), check("b")])],
        &[(
            "",
            &[check("a").needing(&["b"]), check("b").needing(&["a"])],
        )],
        &["a", "b"],
        "",
    )
    .await;
}

/// The job of the check `id` among `jobs`.
///
/// # Panics
///
/// Where there is none.
fn job_of<'a>(jobs: &'a [Job], id: &str) -> &'a Job {
    jobs.iter()
        .find(|job| job.check.id().to_string() == id)
        .unwrap()
}

/// A landing that changes only one folder queues the checks of that folder and records those of
/// the other as passed from the cache, with the output tree of the run that passed; changing the
/// other folder queues its checks instead.
pub async fn a_landing_that_changes_one_folder_queues_only_its_checks<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[("a", &[check("unit")]), ("b", &[check("unit")])];
    let first = land_checks_with(
        &repo,
        &ada,
        "first",
        folders,
        &[("a/src", "1\n"), ("b/src", "1\n")],
    )
    .await;
    run_all(&harness, &bot, &[], 0).await;
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(first));

    let second = land(&repo, &ada, "edit-a", &[("a/src", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, second, 1).await;
    let queued = job_of(&after, "a:unit");
    assert_eq!(queued.state, JobState::Queued);
    let cached = job_of(&after, "b:unit");
    assert_eq!(cached.state, JobState::Cached);
    assert_eq!(cached.output_tree, Some(empty_output_tree()));
    assert!(cached.key.is_some());
    assert_eq!(cached.runner, None);
    // Only the folder that changed is handed to a runner, and `last-green` waits for it.
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(first));
    let claimed = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    assert_eq!(claimed.job.id, queued.id);
    assert!(harness.jobs().claim(&bot, 2).await.unwrap().is_none());
    harness
        .jobs()
        .report(&bot, queued.id, passed(), Produced::default(), 3)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(second));

    // The control: changing the other folder queues its checks.
    let third = land(&repo, &ada, "edit-b", &[("b/src", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, third, 4).await;
    assert_eq!(job_of(&after, "b:unit").state, JobState::Queued);
    assert_eq!(job_of(&after, "a:unit").state, JobState::Cached);
}

/// A check whose key passed is not queued again, however many landings leave its inputs alone: it
/// is passed from the cache and its changeset is `last-green` at once. A check in another
/// repository is not answered from this one's cache.
pub async fn a_check_whose_key_passed_is_not_queued_again<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let other = harness.repository(&ada, "other").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[("lib", &[check("unit")])];
    let files = [("lib/a", "1\n"), ("docs/x", "1\n")];
    land_checks_with(&repo, &ada, "first", folders, &files).await;
    run_all(&harness, &bot, &[], 0).await;

    let second = land(&repo, &ada, "edit-docs", &[("docs/x", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, second, 1).await;
    assert_eq!(job_of(&after, "lib:unit").state, JobState::Cached);
    assert!(harness.jobs().claim(&bot, 1).await.unwrap().is_none());
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(second));

    // The control: changing a file its inputs match queues it.
    let third = land(&repo, &ada, "edit-lib", &[("lib/a", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, third, 2).await;
    assert_eq!(job_of(&after, "lib:unit").state, JobState::Queued);

    // The cache is the repository's own.
    let elsewhere = land_checks_with(&other, &ada, "first", folders, &files).await;
    let after = jobs(&harness, &ada, &other, elsewhere, 3).await;
    assert_eq!(job_of(&after, "lib:unit").state, JobState::Queued);
}

/// A run that did not pass is not cached: the next landing queues the same key again, and once a
/// run has passed the one after it is passed from the cache.
pub async fn a_failed_run_is_not_cached<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[("lib", &[check("lint").non_blocking()])];
    let files = [("lib/a", "1\n"), ("docs/x", "1\n")];
    land_checks_with(&repo, &ada, "first", folders, &files).await;
    run_all(&harness, &bot, &["lint"], 0).await;

    let second = land(&repo, &ada, "edit-docs", &[("docs/x", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, second, 1).await;
    assert_eq!(job_of(&after, "lib:lint").state, JobState::Queued);
    run_all(&harness, &bot, &[], 2).await;

    let third = land(&repo, &ada, "edit-docs-again", &[("docs/x", "3\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, third, 3).await;
    assert_eq!(job_of(&after, "lib:lint").state, JobState::Cached);
}

/// A check is keyed over the checks it needs: where a check it needs runs again because its inputs
/// changed, the check runs again too, though none of its own inputs changed.
pub async fn a_check_is_keyed_over_the_keys_of_the_checks_it_needs<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[
        ("a", &[check("unit")]),
        ("b", &[check("unit").needing(&["//a:unit"])]),
    ];
    let files = [("a/src", "1\n"), ("b/src", "1\n"), ("docs/x", "1\n")];
    land_checks_with(&repo, &ada, "first", folders, &files).await;
    run_all(&harness, &bot, &[], 0).await;

    let second = land(&repo, &ada, "edit-a", &[("a/src", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, second, 1).await;
    assert_eq!(job_of(&after, "a:unit").state, JobState::Queued);
    assert_eq!(job_of(&after, "b:unit").state, JobState::Waiting);
    run_all(&harness, &bot, &[], 2).await;
    let after = jobs(&harness, &ada, &repo, second, 3).await;
    assert_eq!(job_of(&after, "b:unit").state, JobState::Passed);

    let third = land(&repo, &ada, "edit-docs", &[("docs/x", "2\n")])
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, third, 4).await;
    assert_eq!(job_of(&after, "a:unit").state, JobState::Cached);
    assert_eq!(job_of(&after, "b:unit").state, JobState::Cached);
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(third));
}

/// A check that needs two others is queued only once both have passed, and no runner is handed it
/// before.
pub async fn a_check_is_queued_once_every_check_it_needs_has_passed<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let landed = land_checks(
        &repo,
        &ada,
        "order",
        &[check("a"), check("c"), check("b").needing(&["a", "c"])],
    )
    .await;
    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    assert_eq!(job_of(&queued, "a").state, JobState::Queued);
    assert_eq!(job_of(&queued, "c").state, JobState::Queued);
    assert_eq!(job_of(&queued, "b").state, JobState::Waiting);
    let (a, b, c) = (
        job_of(&queued, "a").id,
        job_of(&queued, "b").id,
        job_of(&queued, "c").id,
    );

    let first = harness.jobs().claim(&bot, 1).await.unwrap().unwrap();
    let second = harness.jobs().claim(&bot, 1).await.unwrap().unwrap();
    assert_eq!([first.job.id, second.job.id], [a, c]);
    assert!(harness.jobs().claim(&bot, 1).await.unwrap().is_none());

    harness
        .jobs()
        .report(&bot, a, passed(), Produced::default(), 2)
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, landed, 2).await;
    assert_eq!(job_of(&after, "b").state, JobState::Waiting);
    assert!(harness.jobs().claim(&bot, 2).await.unwrap().is_none());

    harness
        .jobs()
        .report(&bot, c, passed(), Produced::default(), 3)
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, landed, 3).await;
    assert_eq!(job_of(&after, "b").state, JobState::Queued);
    let third = harness.jobs().claim(&bot, 3).await.unwrap().unwrap();
    assert_eq!(third.job.id, b);
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
    harness
        .jobs()
        .report(&bot, b, passed(), Produced::default(), 4)
        .await
        .unwrap();
    assert_eq!(last_green(&harness, &ada, &repo).await, Some(landed));
}

/// A check one of whose needs failed is skipped and never handed to a runner.
pub async fn a_check_whose_need_failed_is_skipped_and_never_queued<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let landed = land_checks(
        &repo,
        &ada,
        "order",
        &[check("a"), check("c"), check("b").needing(&["a", "c"])],
    )
    .await;
    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    let b = job_of(&queued, "b").id;

    let first = harness.jobs().claim(&bot, 1).await.unwrap().unwrap();
    assert_eq!(first.job.check.name.as_str(), "a");
    harness
        .jobs()
        .report(&bot, first.job.id, failed(), Produced::default(), 2)
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, landed, 2).await;
    assert_eq!(job_of(&after, "b").state, JobState::Skipped);
    assert_eq!(job_of(&after, "b").key, None);

    // Nothing is ever handed out under the skipped job, nor does it pass later.
    while let Some(claimed) = harness.jobs().claim(&bot, 3).await.unwrap() {
        assert_ne!(claimed.job.id, b);
        harness
            .jobs()
            .report(&bot, claimed.job.id, passed(), Produced::default(), 3)
            .await
            .unwrap();
    }
    let after = jobs(&harness, &ada, &repo, landed, 4).await;
    assert_eq!(job_of(&after, "b").state, JobState::Skipped);
}

/// A skipped blocking check has not passed, so `last-green` does not move past its changeset, and
/// it reverts nothing: the check that was needed failed without blocking.
pub async fn a_skipped_blocking_check_keeps_last_green_back_and_reverts_nothing<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let landed = land_checks(
        &repo,
        &ada,
        "skipped",
        &[
            check("lint").non_blocking(),
            check("unit").needing(&["lint"]),
        ],
    )
    .await;

    run_all(&harness, &bot, &["lint"], 1).await;

    let after = jobs(&harness, &ada, &repo, landed, 2).await;
    assert_eq!(job_of(&after, "lint").state, JobState::Failed);
    let unit = job_of(&after, "unit");
    assert_eq!(unit.state, JobState::Skipped);
    assert!(unit.check.blocking);
    assert!(after.iter().all(|job| job.revert.is_none()));
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
}

/// Cancelling a check skips the checks that need it.
pub async fn cancelling_a_check_skips_the_checks_that_need_it<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_checks(
        &repo,
        &ada,
        "cancel",
        &[check("a"), check("b").needing(&["a"])],
    )
    .await;
    let queued = jobs(&harness, &ada, &repo, landed, 0).await;
    let a = job_of(&queued, "a").id;

    harness.checks().cancel(&ada, repo.id, a).await.unwrap();

    let after = jobs(&harness, &ada, &repo, landed, 1).await;
    assert_eq!(job_of(&after, "a").state, JobState::Cancelled);
    assert_eq!(job_of(&after, "b").state, JobState::Skipped);
    assert_eq!(last_green(&harness, &ada, &repo).await, None);
}

/// The planned check named `id` among `checks`.
///
/// # Panics
///
/// Where there is none.
fn planned<'a>(checks: &'a [PlannedCheck], id: &str) -> &'a PlannedCheck {
    checks
        .iter()
        .find(|check| check.check.to_string() == id)
        .unwrap()
}

/// A plan names a check whose folder the draft changes as a run for its inputs, one whose folder
/// it leaves alone as cached, and one that needs the first as a run for that need; it queues and
/// lands nothing. A draft that touches no folder's files takes every check from the cache.
pub async fn a_plan_runs_what_the_draft_changes_and_takes_the_rest_from_the_cache<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[
        ("a", &[check("unit")]),
        ("b", &[check("unit")]),
        ("c", &[check("package").needing(&["//a:unit"])]),
    ];
    let first = land_checks_with(
        &repo,
        &ada,
        "first",
        folders,
        &[("a/src", "1\n"), ("b/src", "1\n"), ("c/src", "1\n")],
    )
    .await;
    run_all(&harness, &bot, &[], 0).await;

    // The control: a draft that changes no folder's files caches every check.
    let idle = draft("idle");
    repo.stage(&ada, &idle, &[("README", "hello\n")]).await;
    let plan = harness.checks().plan(&ada, repo.id, &idle).await.unwrap();
    let ids: Vec<String> = plan.checks.iter().map(|c| c.check.to_string()).collect();
    assert_eq!(ids, ["a:unit", "b:unit", "c:package"]);
    assert!(plan.checks.iter().all(|c| c.outcome == PlanOutcome::Cached));

    let work = draft("work");
    repo.stage(&ada, &work, &[("a/src", "2\n")]).await;
    let plan = harness.checks().plan(&ada, repo.id, &work).await.unwrap();
    assert_eq!(
        planned(&plan.checks, "a:unit").outcome,
        PlanOutcome::Run(vec![RunReason::InputsChanged])
    );
    assert_eq!(planned(&plan.checks, "b:unit").outcome, PlanOutcome::Cached);
    let package = planned(&plan.checks, "c:package");
    let a_unit = planned(&plan.checks, "a:unit").check.clone();
    assert_eq!(
        package.outcome,
        PlanOutcome::Run(vec![RunReason::NeedRuns(a_unit)])
    );
    assert!(package.blocking);

    // Planning queued and landed nothing.
    assert!(harness.jobs().claim(&bot, 1).await.unwrap().is_none());
    assert_eq!(repo.main().await, Some(first));
}

/// A check whose key has no passed run is planned as a run with no reason but that, and a draft
/// whose files conflict with `main`, or whose check file cannot be read, has no plan.
pub async fn a_plan_names_a_check_that_never_passed_and_refuses_a_draft_it_cannot_read<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let folders: &[(&str, &[Spec])] = &[("a", &[check("unit")])];
    land_checks_with(&repo, &ada, "first", folders, &[("a/src", "1\n")]).await;

    // No run has passed, though the landing queued one.
    let idle = draft("idle");
    repo.stage(&ada, &idle, &[("README", "hello\n")]).await;
    let plan = harness.checks().plan(&ada, repo.id, &idle).await.unwrap();
    assert_eq!(
        planned(&plan.checks, "a:unit").outcome,
        PlanOutcome::Run(vec![RunReason::NoPassedRun])
    );

    // A draft that conflicts with what `main` changed since its base is refused.
    let late = draft("late");
    repo.stage(&ada, &late, &[("a/src", "2\n")]).await;
    land(&repo, &ada, "other", &[("a/src", "3\n")])
        .await
        .unwrap();
    let refused = harness.checks().plan(&ada, repo.id, &late).await;
    assert!(matches!(
        refused,
        Err(ChecksError::Plan(PlanError::Rebase(LandError::Conflicts(
            _
        ))))
    ));

    // A check file the definitions cannot read is refused, naming it.
    let broken = draft("broken");
    let file = format!("a/{}", harness.definitions().file());
    repo.stage(&ada, &broken, &[(&file, "not taught")]).await;
    let refused = harness.checks().plan(&ada, repo.id, &broken).await;
    assert!(matches!(
        refused,
        Err(ChecksError::Plan(PlanError::Definitions { file: named, .. })) if named == file
    ));
}

/// The paths of the files a claim grants, in order.
fn granted(claimed: &Claimed) -> Vec<String> {
    claimed
        .files
        .iter()
        .map(|file| file.path.to_string())
        .collect()
}

/// Stores `bytes` in `repo`'s object store and returns the file at `path` with them as content.
async fn produce<H: RunnerDeployment>(
    harness: &H,
    repo: &crate::contract::Repo<'_, H>,
    path: &str,
    bytes: &[u8],
) -> ProducedFile {
    ProducedFile {
        path: path.parse().unwrap(),
        file: File {
            kind: FileKind::Regular,
            content: harness
                .objects()
                .put(repo.id, bytes.to_vec())
                .await
                .unwrap(),
        },
    }
}

/// The folder of `claimed`'s check, as a path.
fn folder_of(claimed: &Claimed) -> String {
    String::from_utf8_lossy(claimed.job.check.folder.components()[0].as_bytes()).into_owned()
}

/// A job is granted the files its inputs match at its changeset and no others.
pub async fn a_job_is_granted_the_files_its_inputs_match_and_no_others<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[("web", &[check("unit").reading(&["src/**"])])];
    let files = [
        ("web/src/a", "1\n"),
        ("web/notes", "1\n"),
        ("docs/x", "1\n"),
    ];
    land_checks_with(&repo, &ada, "first", folders, &files).await;

    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    // The file an input matches is granted; the files no input matches are not.
    assert_eq!(granted(&claimed), ["web/src/a"]);
}

/// Claims every queued job as `bot`, each leaving `bytes` under `dist` of its folder besides a
/// stray file, and reports it passed.
async fn run_producing<H: RunnerDeployment>(
    harness: &H,
    repo: &crate::contract::Repo<'_, H>,
    bot: &Caller,
    bytes: &[u8],
    now: i64,
) {
    while let Some(claimed) = harness.jobs().claim(bot, now).await.unwrap() {
        let folder = folder_of(&claimed);
        let produced = Produced {
            outputs: vec![
                produce(harness, repo, &format!("{folder}/dist/out"), bytes).await,
                // A file outside the declared outputs is not part of the output tree.
                produce(harness, repo, &format!("{folder}/stray"), b"stray\n").await,
            ],
            caches: Vec::new(),
        };
        harness
            .jobs()
            .report(bot, claimed.job.id, passed(), produced, now)
            .await
            .unwrap();
    }
}

/// A job is granted the output tree of each check it needs, at the paths the check declares, and
/// of no check it does not need; the output tree is what the passed check reported under its
/// outputs.
pub async fn a_job_is_granted_the_outputs_of_the_checks_it_needs<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[
        ("a", &[check("build").producing(&["dist"])]),
        (
            "b",
            &[check("use").reading(&["src/**"]).needing(&["//a:build"])],
        ),
        ("c", &[check("other").producing(&["dist"])]),
    ];
    let files = [("a/src", "1\n"), ("b/src", "1\n"), ("c/src", "1\n")];
    let landed = land_checks_with(&repo, &ada, "first", folders, &files).await;

    run_producing_first(&harness, &repo, &bot).await;

    let after = jobs(&harness, &ada, &repo, landed, 2).await;
    assert_eq!(job_of(&after, "b:use").state, JobState::Queued);
    let claimed = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    assert_eq!(claimed.job.check.id().to_string(), "b:use");
    // Its own input and the output of the check it needs; not the stray file, nor the output of
    // the check it does not need.
    assert_eq!(granted(&claimed), ["a/dist/out", "b/src"]);
}

/// Runs the two jobs that are queued at first, the third waiting.
async fn run_producing_first<H: RunnerDeployment>(
    harness: &H,
    repo: &crate::contract::Repo<'_, H>,
    bot: &Caller,
) {
    let mut claimed = Vec::new();
    while let Some(next) = harness.jobs().claim(bot, 0).await.unwrap() {
        claimed.push(next);
    }
    assert_eq!(claimed.len(), 2, "b waits for a");
    for next in claimed {
        let folder = folder_of(&next);
        let produced = Produced {
            outputs: vec![
                produce(harness, repo, &format!("{folder}/dist/out"), b"built\n").await,
                produce(harness, repo, &format!("{folder}/stray"), b"stray\n").await,
            ],
            caches: Vec::new(),
        };
        harness
            .jobs()
            .report(bot, next.job.id, passed(), produced, 1)
            .await
            .unwrap();
    }
}

/// A check whose needed check left a different output tree is keyed differently, and one whose
/// needed check is unchanged is not run again.
pub async fn a_check_is_rekeyed_when_the_output_of_a_check_it_needs_changes<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[
        ("a", &[check("build").producing(&["dist"])]),
        ("b", &[check("use").needing(&["//a:build"])]),
    ];
    let files = [("a/src", "1\n"), ("b/src", "1\n"), ("docs/x", "1\n")];
    let first = land_checks_with(&repo, &ada, "first", folders, &files).await;
    run_producing(&harness, &repo, &bot, b"one\n", 0).await;
    let before = jobs(&harness, &ada, &repo, first, 1).await;
    let before = job_of(&before, "b:use").key.unwrap();

    let second = land(&repo, &ada, "edit-a", &[("a/src", "2\n")])
        .await
        .unwrap();
    run_producing(&harness, &repo, &bot, b"two\n", 2).await;
    let after = jobs(&harness, &ada, &repo, second, 3).await;
    assert_eq!(job_of(&after, "b:use").state, JobState::Passed);
    assert_ne!(job_of(&after, "b:use").key.unwrap(), before);

    // The control: a landing that leaves both alone passes both from the cache, with one key.
    let third = land(&repo, &ada, "edit-docs", &[("docs/x", "2\n")])
        .await
        .unwrap();
    let again = jobs(&harness, &ada, &repo, third, 4).await;
    assert_eq!(job_of(&again, "b:use").state, JobState::Cached);
    assert_eq!(job_of(&again, "b:use").key, job_of(&after, "b:use").key);
}

/// A passed report whose files the object store lacks ends the job as failed, and the grants for
/// uploads cover only the files the store lacks.
pub async fn a_pass_with_files_the_store_lacks_is_a_failure<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[("a", &[check("build").producing(&["dist"])])];
    let landed = land_checks_with(&repo, &ada, "first", folders, &[("a/src", "1\n")]).await;
    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();

    let held = produce(&harness, &repo, "a/dist/held", b"held\n").await;
    let lacking = Upload {
        address: Address::of(b"lacking\n"),
        size: 8,
    };
    let held_upload = Upload {
        address: held.file.content,
        size: 5,
    };
    let grants = harness
        .jobs()
        .upload_grants(&bot, claimed.job.id, vec![held_upload, lacking], 0)
        .await
        .unwrap();
    let addresses: Vec<Address> = grants.iter().map(|grant| grant.address).collect();
    assert_eq!(addresses, [lacking.address], "the store holds the other");

    let never_uploaded = ProducedFile {
        path: "a/dist/lacking".parse().unwrap(),
        file: File {
            kind: FileKind::Regular,
            content: lacking.address,
        },
    };
    let produced = Produced {
        outputs: vec![held, never_uploaded],
        caches: Vec::new(),
    };
    harness
        .jobs()
        .report(&bot, claimed.job.id, passed(), produced, 1)
        .await
        .unwrap();
    let after = jobs(&harness, &ada, &repo, landed, 2).await;
    assert_eq!(job_of(&after, "a:build").state, JobState::Failed);
}

/// The cache ref of `path` of the check at `folder` in `repo`.
async fn cache_ref_of<H: RunnerDeployment>(
    harness: &H,
    repo: &crate::contract::Repo<'_, H>,
    folder: &str,
    path: &str,
) -> Option<CacheRef> {
    let folder = Prefix::from(folder.parse::<Path>().unwrap());
    harness
        .queue()
        .cache_ref(repo.id, &folder, &path.parse().unwrap())
        .await
        .unwrap()
}

/// Reports `claimed` as `outcome`, leaving `bytes` in `lib/target`.
async fn report_caching<H: RunnerDeployment>(
    harness: &H,
    repo: &crate::contract::Repo<'_, H>,
    bot: &Caller,
    claimed: &Claimed,
    outcome: Outcome,
    bytes: &[u8],
) {
    let result = if outcome == Outcome::Passed {
        passed()
    } else {
        failed()
    };
    let produced = Produced {
        outputs: Vec::new(),
        caches: vec![produce(harness, repo, "lib/target/out", bytes).await],
    };
    harness
        .jobs()
        .report(bot, claimed.job.id, result, produced, 1)
        .await
        .unwrap();
}

/// A cache ref moves to the tree a passed job left only where the job's changeset is later on
/// `main` than the ref's; the next job of the check is granted that tree at the cache path.
pub async fn a_cache_ref_advances_only_on_a_pass_at_a_later_landing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let folders: &[(&str, &[Spec])] = &[(
        "lib",
        &[check("build").reading(&["src/**"]).caching(&["target"])],
    )];
    let c1 = land_checks_with(&repo, &ada, "first", folders, &[("lib/src/a", "1\n")]).await;
    assert_eq!(cache_ref_of(&harness, &repo, "lib", "target").await, None);
    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    assert_eq!(granted(&claimed), ["lib/src/a"], "no ref, so no cache");
    report_caching(&harness, &repo, &bot, &claimed, Outcome::Passed, b"one\n").await;
    let one = cache_ref_of(&harness, &repo, "lib", "target")
        .await
        .unwrap();
    assert_eq!(
        one.changeset, c1,
        "a pass at the first landing sets the ref"
    );

    // A pass at a later landing advances it, and the job is granted the tree it points at.
    let c2 = land(&repo, &ada, "second", &[("lib/src/a", "2\n")])
        .await
        .unwrap();
    let claimed = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    assert_eq!(granted(&claimed), ["lib/src/a", "lib/target/out"]);
    report_caching(&harness, &repo, &bot, &claimed, Outcome::Passed, b"two\n").await;
    let two = cache_ref_of(&harness, &repo, "lib", "target")
        .await
        .unwrap();
    assert_eq!(two.changeset, c2);
    assert_ne!(two.tree, one.tree);

    // A failed job at a later landing leaves it.
    land(&repo, &ada, "third", &[("lib/src/a", "3\n")])
        .await
        .unwrap();
    let claimed = harness.jobs().claim(&bot, 3).await.unwrap().unwrap();
    report_caching(&harness, &repo, &bot, &claimed, Outcome::Failed, b"three\n").await;
    assert_eq!(
        cache_ref_of(&harness, &repo, "lib", "target").await,
        Some(two)
    );

    // A pass at an earlier landing than the ref's leaves it; the later one advances it.
    let c4 = land(&repo, &ada, "fourth", &[("lib/src/a", "4\n")])
        .await
        .unwrap();
    let c5 = land(&repo, &ada, "fifth", &[("lib/src/a", "5\n")])
        .await
        .unwrap();
    let mut queued = Vec::new();
    while let Some(next) = harness.jobs().claim(&bot, 4).await.unwrap() {
        queued.push(next);
    }
    let at = |changeset: Address| {
        queued
            .iter()
            .find(|job| job.job.changeset == changeset)
            .unwrap()
    };
    report_caching(&harness, &repo, &bot, at(c5), Outcome::Passed, b"five\n").await;
    let five = cache_ref_of(&harness, &repo, "lib", "target")
        .await
        .unwrap();
    assert_eq!(five.changeset, c5);
    report_caching(&harness, &repo, &bot, at(c4), Outcome::Passed, b"four\n").await;
    assert_eq!(
        cache_ref_of(&harness, &repo, "lib", "target").await,
        Some(five)
    );
}

/// A cache ref is set where it has none, and moves only from the changeset it records.
pub async fn a_cache_ref_moves_only_from_the_changeset_it_records<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let folder = Prefix::from("lib".parse::<Path>().unwrap());
    let path: Path = "target".parse().unwrap();
    let to = |tree: &[u8], changeset: &[u8]| CacheRef {
        tree: Address::of(tree),
        changeset: Address::of(changeset),
    };
    let queue = harness.queue();

    let first = to(b"t1", b"c1");
    let set = queue
        .advance_cache_ref(repo.id, &folder, &path, None, first)
        .await
        .unwrap();
    assert!(set);
    // Control: moving from the changeset it records succeeds.
    let second = to(b"t2", b"c2");
    let moved = queue
        .advance_cache_ref(repo.id, &folder, &path, Some(first.changeset), second)
        .await
        .unwrap();
    assert!(moved);
    // Moving from one it does not record, or from none, does not.
    let third = to(b"t3", b"c3");
    let stale = queue
        .advance_cache_ref(repo.id, &folder, &path, Some(first.changeset), third)
        .await
        .unwrap();
    assert!(!stale);
    let blind = queue
        .advance_cache_ref(repo.id, &folder, &path, None, third)
        .await
        .unwrap();
    assert!(!blind);
    let held = queue.cache_ref(repo.id, &folder, &path).await.unwrap();
    assert_eq!(held, Some(second));
    // The ref is of its folder and path.
    let other = Prefix::from("app".parse::<Path>().unwrap());
    let elsewhere = queue.cache_ref(repo.id, &other, &path).await.unwrap();
    assert_eq!(elsewhere, None);
}
