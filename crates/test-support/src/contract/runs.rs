//! Runs started in a workspace (ADR 0051), through the runs port: the permission to start one, the
//! settings file, the cache refs a start seeds, and who may read, cancel and list a run.

use std::collections::{BTreeMap, BTreeSet};

use crate::contract::access::Admin;
use crate::contract::runner_deployment::RunnerDeployment;
use crate::contract::{Repo, draft};
use app::domain::address::Address;
use app::domain::check::{Command, Timeout};
use app::domain::draft::DraftId;
use app::domain::job::{CacheRef, empty_output_tree};
use app::domain::path::{Path, Prefix};
use app::domain::principal::Caller;
use app::domain::run::{Run, RunId, RunSettings, RunState};
use app::port::driven::cache_refs::CacheRefs;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::draft_store::DraftStore;
use app::port::driven::membership::{Access, Role};
use app::port::driving::access::AccessError;
use app::port::driving::runs::{Runs, RunsError, StartRun};

const RUN_FILE: &str = ".codehub/run.toml";

fn path(text: &str) -> Path {
    text.parse().unwrap()
}

fn paths(texts: &[&str]) -> BTreeSet<Path> {
    texts.iter().map(|text| path(text)).collect()
}

fn request(prefix: Option<&str>, add: &[&str]) -> StartRun {
    StartRun {
        command: Command::new("make").unwrap(),
        prefix: prefix.map(|prefix| app::domain::path::Prefix::from(path(prefix))),
        add: paths(add),
        outputs: paths(&["out"]),
    }
}

fn denied(access: Access, texts: &[&str]) -> RunsError {
    RunsError::Access(AccessError::PathsDenied {
        access,
        paths: paths(texts),
    })
}

async fn start<H: RunnerDeployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    request: StartRun,
) -> Result<Run, RunsError> {
    repo.harness
        .assembly()
        .runs()
        .start(caller, repo.id, &draft("work"), request)
        .await
}

/// A caller without `run` on the repository is refused and no run is queued; with it, the run
/// records the files, the instance and the version of the workspace, limited to the prefix.
pub async fn starting_a_run_needs_the_run_action<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(&bob, &draft("work"), &[("src/a", "a"), ("docs/b", "b")])
        .await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    // Control: the owner's run takes the next identifier.
    let first = start(&repo, &ada, request(None, &[])).await.unwrap();

    let refused = start(&repo, &bob, request(None, &[])).await;
    assert_eq!(
        refused,
        Err(RunsError::Access(AccessError::Denied(
            app::port::driven::authorisation_decision::Action::Run
        )))
    );

    admin.grant(&bob, "", Access::Run).await;
    let run = start(&repo, &bob, request(Some("src"), &["src/new"]))
        .await
        .unwrap();
    // The refusal queued nothing: it took no identifier.
    assert_eq!(run.id, RunId::new(first.id.value() + 1));
    assert_eq!(run.state, RunState::Queued);
    assert_eq!(run.request.principal, *bob.principal());
    let held = harness
        .drafts()
        .get(
            repo.id,
            &DraftId {
                principal: bob.principal().clone(),
                name: draft("work"),
            },
        )
        .await
        .unwrap();
    assert_eq!(run.request.version, held.version);
    assert_eq!(run.request.instance, held.instance);
    assert_eq!(run.request.base, held.base);
    assert_eq!(
        run.request.files.keys().cloned().collect::<Vec<_>>(),
        vec![path("src/a")]
    );
    assert!(run.request.labels.is_empty());
    assert_eq!(run.request.timeout, Timeout::default_timeout());
    assert!(run.request.caches.is_empty());
}

/// A path outside the prefix cannot be added, and the start queues nothing.
pub async fn an_add_path_outside_the_prefix_is_refused<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let control = start(&repo, &ada, request(Some("src"), &["src/x"]))
        .await
        .unwrap();

    let refused = start(&repo, &ada, request(Some("src"), &["docs/x"])).await;

    assert_eq!(refused, Err(RunsError::AddOutsidePrefix(path("docs/x"))));
    let next = start(&repo, &ada, request(None, &[])).await.unwrap();
    assert_eq!(next.id, RunId::new(control.id.value() + 1));
}

/// Where the start sends a path the caller may not read, or the settings file, it is refused.
pub async fn every_path_sent_and_the_settings_file_must_be_readable<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(
        &bob,
        &draft("work"),
        &[
            ("src/a", "a"),
            ("docs/b", "b"),
            (RUN_FILE, "timeout = oops"),
        ],
    )
    .await;
    harness.teach_run(
        "timeout = oops",
        Err(CheckFileError::Invalid {
            line: 1,
            reason: "timeout is a whole number of seconds".to_owned(),
        }),
    );
    admin.grant(&bob, "", Access::Run).await;
    admin.ungrant(&bob, "", Access::Write).await;
    admin.grant(&bob, "src", Access::Read).await;

    // The workspace tree holds the settings file, which the caller may not read: the refusal is
    // the same whether or not it parses, and says nothing of its content.
    assert_eq!(
        start(&repo, &bob, request(Some("src"), &[])).await,
        Err(denied(Access::Read, &[RUN_FILE]))
    );
    assert_eq!(
        start(&repo, &bob, request(None, &[])).await,
        Err(denied(Access::Read, &[RUN_FILE, "docs/b"]))
    );

    // With read on it, the file is parsed.
    admin.grant(&bob, ".codehub", Access::Read).await;
    let parsed = start(&repo, &bob, request(Some("src"), &[])).await;
    assert!(
        matches!(parsed, Err(RunsError::RunFile { .. })),
        "{parsed:?}"
    );

    // Control: with read on the settings file and the prefix, a file that parses queues the run,
    // with its timeout.
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(&bob, &draft("work"), &[(RUN_FILE, "timeout = 5")])
        .await;
    harness.teach_run("timeout = 5", Ok(settings(&[], 5)));
    let run = start(&repo, &bob, request(Some("src"), &[])).await.unwrap();
    assert_eq!(run.request.timeout, Timeout::new(5).unwrap());
}

fn settings(caches: &[&str], timeout: u64) -> RunSettings {
    RunSettings {
        caches: paths(caches),
        labels: BTreeSet::new(),
        timeout: Timeout::new(timeout).unwrap(),
    }
}

/// A settings file that does not parse is refused with the file named, and an absent one means the
/// defaults.
pub async fn a_settings_file_that_does_not_parse_is_refused<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let absent = start(&repo, &ada, request(None, &[])).await.unwrap();
    assert_eq!(absent.request.timeout, Timeout::default_timeout());

    repo.stage(&ada, &draft("work"), &[(RUN_FILE, "timeout = oops")])
        .await;
    harness.teach_run(
        "timeout = oops",
        Err(CheckFileError::Invalid {
            line: 1,
            reason: "timeout is a whole number of seconds".to_owned(),
        }),
    );

    let refused = start(&repo, &ada, request(None, &[])).await;

    assert!(
        matches!(&refused, Err(RunsError::RunFile { file, .. }) if *file == path(RUN_FILE)),
        "{refused:?}"
    );
    // The refusal queued nothing: once the file reads, the run takes the next identifier, and
    // the settings file is read whatever the prefix.
    repo.stage(&ada, &draft("work"), &[(RUN_FILE, "timeout = 7")])
        .await;
    harness.teach_run("timeout = 7", Ok(settings(&[], 7)));
    let next = start(&repo, &ada, request(Some("src"), &[])).await.unwrap();
    assert_eq!(next.id, RunId::new(absent.id.value() + 1));
    assert_eq!(next.request.timeout, Timeout::new(7).unwrap());
}

/// A member who reads only `src` and `.codehub`, runs, and has a workspace that caches `target`,
/// where `main` has a file in `docs`.
async fn limited_cache_reader<H: RunnerDeployment>(
    harness: &H,
    ada: &Caller,
    bob: &Caller,
    repo: &Repo<'_, H>,
) {
    let admin = Admin {
        harness,
        caller: ada,
    };
    repo.stage(ada, &draft("landed"), &[("docs/b", "b")]).await;
    repo.land(ada, &draft("landed"), "main").await.unwrap();
    admin.add(bob, Role::Member).await;
    admin.grant(bob, "", Access::Write).await;
    repo.stage(
        bob,
        &draft("work"),
        &[("src/a", "a"), (RUN_FILE, "caches = [\"target\"]")],
    )
    .await;
    harness.teach_run("caches = [\"target\"]", Ok(settings(&["target"], 600)));
    admin.grant(bob, "", Access::Run).await;
    admin.ungrant(bob, "", Access::Write).await;
    admin.grant(bob, "src", Access::Read).await;
    admin.grant(bob, ".codehub", Access::Read).await;
}

/// The tree of `caller`'s workspace cache ref of `target`, where it has one.
async fn workspace_cache_ref<H: RunnerDeployment>(
    harness: &H,
    repo: &Repo<'_, H>,
    caller: &Caller,
) -> Option<Address> {
    let held = harness
        .drafts()
        .get(
            repo.id,
            &DraftId {
                principal: caller.principal().clone(),
                name: draft("work"),
            },
        )
        .await
        .unwrap();
    harness
        .cache_refs()
        .workspace_cache_ref(repo.id, held.instance, &path("target"))
        .await
        .unwrap()
}

/// A cache the workspace has no ref for is seeded from `main`'s ref, which needs read on all of
/// `main`: a caller who may not read it is refused and nothing is seeded or queued.
pub async fn seeding_a_cache_needs_read_on_the_whole_repository<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    limited_cache_reader(&harness, &ada, &bob, &repo).await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let first = start(&repo, &ada, request(None, &[])).await.unwrap();
    let main = CacheRef {
        tree: Address::of(b"main tree"),
        changeset: Address::of(b"landed"),
    };
    let set = harness
        .cache_refs()
        .advance_cache_ref(repo.id, &Prefix::whole(), &path("target"), None, main)
        .await
        .unwrap();
    assert!(set);

    let refused = start(&repo, &bob, request(Some("src"), &[])).await;

    assert_eq!(refused, Err(denied(Access::Read, &["docs/b"])));
    assert_eq!(workspace_cache_ref(&harness, &repo, &bob).await, None);
    // Control: with read on all of `main`, the run is queued with the tree of `main`'s ref, which
    // the workspace's ref now holds. The refusal queued nothing.
    Admin {
        harness: &harness,
        caller: &ada,
    }
    .grant(&bob, "docs", Access::Read)
    .await;
    let run = start(&repo, &bob, request(Some("src"), &[])).await.unwrap();
    assert_eq!(run.id, RunId::new(first.id.value() + 1));
    assert_eq!(
        run.request.caches,
        BTreeMap::from([(path("target"), main.tree)])
    );
    assert_eq!(
        workspace_cache_ref(&harness, &repo, &bob).await,
        Some(main.tree)
    );
}

/// Where `main` has no ref for a cache, the workspace's starts empty and a caller who reads only
/// part of the repository is not refused.
pub async fn a_cache_main_has_no_ref_for_starts_empty_without_a_read_check<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    limited_cache_reader(&harness, &ada, &bob, &repo).await;

    let run = start(&repo, &bob, request(Some("src"), &[])).await.unwrap();

    assert_eq!(
        run.request.caches,
        BTreeMap::from([(path("target"), empty_output_tree())])
    );
    assert_eq!(
        workspace_cache_ref(&harness, &repo, &bob).await,
        Some(empty_output_tree())
    );
}

/// Only the principal that started a run reads, cancels or lists the files of it; to anyone else it
/// is not found, as an unknown run is.
pub async fn another_principals_run_is_not_found<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Read).await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    let runs = harness.assembly().runs();
    let whole = app::domain::path::Prefix::whole();

    assert_eq!(
        runs.get(&bob, repo.id, run.id).await,
        Err(RunsError::NotFound(run.id))
    );
    assert_eq!(
        runs.cancel(&bob, repo.id, run.id).await,
        Err(RunsError::NotFound(run.id))
    );
    assert_eq!(
        runs.files(&bob, repo.id, run.id, &whole).await,
        Err(RunsError::NotFound(run.id))
    );
    let unknown = RunId::new(run.id.value() + 1000);
    assert_eq!(
        runs.get(&ada, repo.id, unknown).await,
        Err(RunsError::NotFound(unknown))
    );
    // Control: the starting principal reads it, still queued because bob could not cancel it.
    assert_eq!(runs.get(&ada, repo.id, run.id).await, Ok(run.clone()));
    assert_eq!(runs.files(&ada, repo.id, run.id, &whole).await, Ok(vec![]));
}

/// Cancelling ends a run, once, and the latest run of a workspace is the latest that has ended.
pub async fn cancelling_ends_a_run_and_the_latest_is_the_latest_ended<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let runs = harness.assembly().runs();
    let first = start(&repo, &ada, request(None, &[])).await.unwrap();
    let second = start(&repo, &ada, request(None, &[])).await.unwrap();
    assert_eq!(
        runs.latest(&ada, repo.id, &draft("work")).await,
        Err(RunsError::NoRun(draft("work")))
    );

    // The later run ends first: the latest is the highest identifier among the ended, not the one
    // that ended last.
    let ended = runs.cancel(&ada, repo.id, second.id).await.unwrap();
    assert_eq!(
        runs.latest(&ada, repo.id, &draft("work")).await,
        Ok(ended.clone())
    );

    let cancelled = runs.cancel(&ada, repo.id, first.id).await.unwrap();

    assert_eq!(cancelled.state, RunState::Cancelled);
    assert!(cancelled.state.ended());
    assert_eq!(
        runs.get(&ada, repo.id, first.id).await,
        Ok(cancelled.clone())
    );
    assert_eq!(
        runs.cancel(&ada, repo.id, first.id).await,
        Err(RunsError::Finished(first.id))
    );
    assert_eq!(runs.latest(&ada, repo.id, &draft("work")).await, Ok(ended));
}
