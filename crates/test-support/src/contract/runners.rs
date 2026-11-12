//! Registering runners and what a runner may claim.

use crate::contract::runner_deployment::{
    RunnerDeployment, check, definitions, labels, land, land_checks, organisation, runner, text,
};
use app::domain::job::{Heartbeat, JobId};
use app::port::driven::check_definitions::CheckDefinitions;
use app::port::driving::jobs::{Jobs, JobsError};
use app::port::driving::repositories::Repositories;
use app::port::driving::runners::{Runners, RunnersError};

/// An organisation administrator registers a runner, lists it, and removes it, after which the
/// runner is refused.
pub async fn an_administrator_registers_lists_and_removes_a_runner<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;

    let id = harness
        .runners()
        .register(&ada, &acme, None, &H::principal("bot"), labels(&["linux"]))
        .await
        .unwrap();
    let listed = harness.runners().list(&ada, &acme).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].registered.principal, H::principal("bot"));
    assert_eq!(listed[0].registered.labels, labels(&["linux"]));

    let bot = harness.caller("bot").await;
    assert!(harness.jobs().claim(&bot, 0).await.unwrap().is_some());

    harness.runners().remove(&ada, &acme, id).await.unwrap();
    assert!(
        harness
            .runners()
            .list(&ada, &acme)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        harness.jobs().claim(&bot, 1).await.unwrap_err(),
        JobsError::NotRegistered
    );
}

/// Registrations are numbered by the registry, each distinct.
pub async fn each_registration_gets_its_own_identifier<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    let first = harness
        .runners()
        .register(&ada, &acme, None, &H::principal("one"), labels(&[]))
        .await
        .unwrap();
    let second = harness
        .runners()
        .register(&ada, &acme, None, &H::principal("two"), labels(&[]))
        .await
        .unwrap();
    assert_ne!(first, second);
}

/// A caller who does not administer the organisation may not register, list or remove runners.
pub async fn a_caller_who_does_not_administer_may_not_manage_runners<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    let id = harness
        .runners()
        .register(&ada, &acme, None, &H::principal("bot"), labels(&[]))
        .await
        .unwrap();
    let eve = harness.caller("eve").await;

    let register = harness
        .runners()
        .register(&eve, &acme, None, &H::principal("eve"), labels(&[]))
        .await;
    assert!(
        matches!(register, Err(RunnersError::Access(_))),
        "{register:?}"
    );
    let list = harness.runners().list(&eve, &acme).await;
    assert!(matches!(list, Err(RunnersError::Access(_))), "{list:?}");
    let remove = harness.runners().remove(&eve, &acme, id).await;
    assert!(matches!(remove, Err(RunnersError::Access(_))), "{remove:?}");
    assert_eq!(harness.runners().list(&ada, &acme).await.unwrap().len(), 1);
}

/// A principal nobody registered is refused every job operation.
pub async fn an_unregistered_principal_is_refused<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    // Ada administers the organisation but is no runner.
    assert_eq!(
        harness.jobs().claim(&ada, 0).await.unwrap_err(),
        JobsError::NotRegistered
    );
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap().job.id;
    assert_eq!(
        harness.jobs().heartbeat(&ada, job, 1).await.unwrap_err(),
        JobsError::NotRegistered
    );
}

/// A runner claims only the jobs whose labels it offers.
pub async fn a_runner_claims_only_jobs_whose_labels_it_offers<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    land_checks(
        &repo,
        &ada,
        "checks",
        &[
            check("gpu-test").labelled(&["gpu"]),
            check("unit").labelled(&["linux"]),
        ],
    )
    .await;
    let linux = runner(&harness, &ada, &acme, "linux-bot", &["linux", "extra"]).await;

    let claimed = harness.jobs().claim(&linux, 0).await.unwrap().unwrap();
    assert_eq!(claimed.job.check.name.as_str(), "unit");
    assert!(harness.jobs().claim(&linux, 0).await.unwrap().is_none());

    let gpu = runner(&harness, &ada, &acme, "gpu-bot", &["gpu"]).await;
    let claimed = harness.jobs().claim(&gpu, 0).await.unwrap().unwrap();
    assert_eq!(claimed.job.check.name.as_str(), "gpu-test");
}

/// A job claimed by one runner is not offered to another, and only its holder keeps it alive.
pub async fn a_claimed_job_is_not_offered_to_another_runner<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let one = runner(&harness, &ada, &acme, "one", &[]).await;
    let two = runner(&harness, &ada, &acme, "two", &[]).await;

    let job = harness.jobs().claim(&one, 0).await.unwrap().unwrap().job.id;
    assert!(harness.jobs().claim(&two, 0).await.unwrap().is_none());
    let stolen = harness.jobs().heartbeat(&two, job, 1).await;
    assert!(stolen.is_err(), "{stolen:?}");
    assert_eq!(
        harness.jobs().heartbeat(&one, job, 1).await.unwrap(),
        Heartbeat::Continue
    );
    assert_ne!(job, JobId::new(0));
}

/// A registration for one repository claims no job of another repository, and a registration of
/// one organisation claims no job of another organisation.
pub async fn a_runner_claims_only_jobs_within_its_scope<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let acme = organisation("acme");
    let other = "other".parse().unwrap();
    harness
        .repositories()
        .create_repository(&ada, &acme, &other)
        .await
        .unwrap();
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;

    // Registered for `other` only, in the organisation that holds the job.
    harness
        .runners()
        .register(
            &ada,
            &acme,
            Some(&other),
            &H::principal("scoped"),
            labels(&[]),
        )
        .await
        .unwrap();
    let scoped = harness.caller("scoped").await;
    assert!(harness.jobs().claim(&scoped, 0).await.unwrap().is_none());

    // Registered for the whole of a different organisation.
    let bea = harness.caller("bea").await;
    harness.repository(&bea, "beta").await;
    let outsider = runner(&harness, &bea, &organisation("beta"), "outsider", &[]).await;
    assert!(harness.jobs().claim(&outsider, 0).await.unwrap().is_none());

    // The organisation-wide registration claims it.
    let wide = runner(&harness, &ada, &acme, "wide", &[]).await;
    assert!(harness.jobs().claim(&wide, 0).await.unwrap().is_some());
}

/// A claim lists the files of the changeset with a download for each.
pub async fn a_claim_lists_the_files_of_the_changeset<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let specs = [check("unit")];
    let file = harness.definitions().file().to_string();
    let body = text(&specs);
    harness.teach(&body, Ok(definitions(&specs)));
    land(
        &repo,
        &ada,
        "work",
        &[(&file, &body), ("src/a.txt", "hello")],
    )
    .await
    .unwrap();
    let bot = runner(&harness, &ada, &organisation("acme"), "bot", &[]).await;

    let claimed = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();
    let paths: Vec<String> = claimed.files.iter().map(|f| f.path.to_string()).collect();
    assert!(paths.contains(&"src/a.txt".to_owned()), "{paths:?}");
    assert!(paths.contains(&file), "{paths:?}");
    let a = claimed
        .files
        .iter()
        .find(|f| f.path.to_string() == "src/a.txt")
        .unwrap();
    assert_eq!(a.size, 5);
}
