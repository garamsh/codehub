//! Runs started in a workspace (ADR 0051), through the runs port: the permission to start one, the
//! settings file, the cache refs a start seeds, and who may read, cancel and list a run.

use std::collections::{BTreeMap, BTreeSet};

use crate::contract::access::Admin;
use crate::contract::runner_deployment::{
    LEASE, RunnerDeployment, check, land_checks, organisation, runner,
};
use crate::contract::{Repo, draft};
use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::check::{Command, Timeout};
use app::domain::draft::Draft;
use app::domain::draft::DraftId;
use app::domain::file::{File, FileKind};
use app::domain::job::{
    CacheRef, Heartbeat, JobId, JobResult, Outcome, Produced, ProducedFile, empty_output_tree,
};
use app::domain::path::{Path, Prefix};
use app::domain::principal::Caller;
use app::domain::run::{
    NotWritten, ObjectCounts, Run, RunId, RunOutcome, RunReport, RunResult, RunSettings, RunState,
    RunStream, WriteBack,
};
use app::port::driven::cache_refs::CacheRefs;
use app::port::driven::check_definitions::CheckFileError;
use app::port::driven::draft_store::DraftStore;
use app::port::driven::job_queue::{JobQueue, JobQueueError};
use app::port::driven::membership::{Access, Role};
use app::port::driving::access::AccessError;
use app::port::driving::jobs::{Jobs, JobsError, Upload};
use app::port::driving::runners::Runners;
use app::port::driving::runs::{Runs, RunsError, StartRun, StreamDownloads};

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

// A runner reporting a run (ADR 0051): what the server writes back, which cache refs it moves, and
// what it lists of the output tree.

fn passed() -> JobResult {
    JobResult {
        outcome: Outcome::Passed,
        exit_code: Some(0),
        output: b"ok".to_vec(),
    }
}

fn regular(content: Address) -> File {
    File {
        kind: FileKind::Regular,
        content,
    }
}

fn exited(changes: Vec<(&str, Change)>) -> RunReport {
    RunReport {
        outcome: RunOutcome::Exited,
        exit_code: Some(0),
        stdout: None,
        stderr: None,
        objects: ObjectCounts::default(),
        outputs: Vec::new(),
        caches: Vec::new(),
        changes: changes
            .into_iter()
            .map(|(text, change)| (path(text), change))
            .collect(),
        refused: Vec::new(),
    }
}

/// A repository whose `main` holds `base`, a runner of its organisation, and the principal's
/// workspace `work` with `staged` in it.
async fn runnable<'a, H: RunnerDeployment>(
    harness: &'a H,
    ada: &Caller,
    base: &[(&str, &str)],
    staged: &[(&str, &str)],
) -> (Repo<'a, H>, Caller) {
    let repo = harness.repository(ada, "acme").await;
    repo.stage(ada, &draft("base"), base).await;
    repo.land(ada, &draft("base"), "main").await.unwrap();
    repo.stage(ada, &draft("work"), staged).await;
    let bot = runner(harness, ada, &organisation("acme"), "bot", &[]).await;
    (repo, bot)
}

async fn workspace<H: RunnerDeployment>(harness: &H, repo: &Repo<'_, H>, caller: &Caller) -> Draft {
    harness
        .drafts()
        .get(
            repo.id,
            &DraftId {
                principal: caller.principal().clone(),
                name: draft("work"),
            },
        )
        .await
        .unwrap()
}

/// The run `id` as its principal reads it.
async fn read<H: RunnerDeployment>(
    harness: &H,
    repo: &Repo<'_, H>,
    caller: &Caller,
    id: RunId,
) -> Run {
    harness
        .assembly()
        .runs()
        .get(caller, repo.id, id)
        .await
        .unwrap()
}

/// The result of `run`, which has ended.
fn result_of(run: &Run) -> RunResult {
    run.ended().expect("the run has ended")
}

fn written(texts: &[&str]) -> WriteBack {
    WriteBack::Written {
        paths: texts.iter().map(|text| path(text)).collect(),
    }
}

fn not_written(reason: NotWritten, texts: &[&str]) -> WriteBack {
    WriteBack::NotWritten {
        reason,
        paths: texts.iter().map(|text| path(text)).collect(),
    }
}

/// An exited run's modified, deleted and added files are written to the workspace as one
/// snapshot, and a change that puts a path back as `main` holds it, or deletes what `main` lacks,
/// removes the workspace's change instead of recording one.
pub async fn an_exited_run_writes_its_changes_to_the_workspace<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a"), ("src/b", "b"), ("src/c", "c")],
        &[("src/c", "edited"), ("src/z", "z")],
    )
    .await;
    let run = start(&repo, &ada, request(None, &["src"])).await.unwrap();
    let before = workspace(&harness, &repo, &ada).await;
    let claimed = harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    assert_eq!(claimed.run.id, run.id);
    let a2 = repo.upload("a2").await;
    let new = repo.upload("new").await;
    let c = repo.upload("c").await;

    let report = exited(vec![
        ("src/a", Change::File(regular(a2))),
        ("src/b", Change::Deletion),
        ("src/new", Change::File(regular(new))),
        ("src/c", Change::File(regular(c))),
        ("src/z", Change::Deletion),
    ]);
    harness
        .jobs()
        .report_run(&bot, run.id, report, 1)
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(ended.state, RunState::Finished);
    assert_eq!(
        result_of(&ended).write_back,
        written(&["src/a", "src/b", "src/c", "src/new", "src/z"])
    );
    let after = workspace(&harness, &repo, &ada).await;
    assert_eq!(after.version, before.version + 1);
    assert_eq!(
        after.changes,
        BTreeMap::from([
            (path("src/a"), Change::File(regular(a2))),
            (path("src/b"), Change::Deletion),
            (path("src/new"), Change::File(regular(new))),
        ])
    );
}

/// A run that changed nothing is written, with no paths, whatever became of the workspace.
pub async fn a_run_that_changed_nothing_is_written_though_the_workspace_moved<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    repo.stage(&ada, &draft("work"), &[("src/a", "y")]).await;
    let moved = workspace(&harness, &repo, &ada).await;

    harness
        .jobs()
        .report_run(&bot, run.id, exited(vec![]), 1)
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(result_of(&ended).write_back, written(&[]));
    assert_eq!(workspace(&harness, &repo, &ada).await, moved);
}

/// Where the workspace changed since the run started, nothing is written and the paths that
/// differ are named.
pub async fn a_moved_workspace_takes_no_write_and_the_paths_are_named<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a"), ("src/b", "b")],
        &[("src/a", "x")],
    )
    .await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    repo.stage(&ada, &draft("work"), &[("src/a", "y")]).await;
    let moved = workspace(&harness, &repo, &ada).await;
    let new = repo.upload("new").await;

    harness
        .jobs()
        .report_run(
            &bot,
            run.id,
            exited(vec![("src/b", Change::File(regular(new)))]),
            1,
        )
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::WorkspaceMoved, &["src/a"])
    );
    assert_eq!(workspace(&harness, &repo, &ada).await, moved);
}

/// A principal who lost write on a changed path has the whole write refused, with the paths she
/// may not write named, and nothing is written.
pub async fn a_denied_write_writes_nothing_and_names_the_paths<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a"), ("docs/b", "b")], &[]).await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Write).await;
    repo.stage(&bob, &draft("work"), &[("src/a", "x")]).await;
    admin.grant(&bob, "", Access::Run).await;
    let run = start(&repo, &bob, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    admin.ungrant(&bob, "", Access::Write).await;
    admin.grant(&bob, "src", Access::Write).await;
    let before = workspace(&harness, &repo, &bob).await;
    let new = repo.upload("new").await;
    let other = repo.upload("other").await;

    harness
        .jobs()
        .report_run(
            &bot,
            run.id,
            exited(vec![
                ("src/a", Change::File(regular(new))),
                ("docs/b", Change::File(regular(other))),
            ]),
            1,
        )
        .await
        .unwrap();

    let ended = read(&harness, &repo, &bob, run.id).await;
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::WriteDenied, &["docs/b"])
    );
    assert_eq!(workspace(&harness, &repo, &bob).await, before);
}

/// A run that timed out, or whose executor failed, is reported with what it has and writes
/// nothing back.
pub async fn a_run_that_did_not_exit_writes_nothing<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let before = workspace(&harness, &repo, &ada).await;
    let new = repo.upload("new").await;
    for (outcome, reason) in [
        (RunOutcome::TimedOut, NotWritten::TimedOut),
        (RunOutcome::ExecutorFailed, NotWritten::ExecutorFailed),
    ] {
        let run = start(&repo, &ada, request(None, &[])).await.unwrap();
        harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
        let mut report = exited(vec![("src/a", Change::File(regular(new)))]);
        report.outcome = outcome;
        report.exit_code = None;

        harness
            .jobs()
            .report_run(&bot, run.id, report, 1)
            .await
            .unwrap();

        let ended = read(&harness, &repo, &ada, run.id).await;
        assert_eq!(result_of(&ended).outcome, outcome);
        assert_eq!(result_of(&ended).write_back, not_written(reason, &[]));
        assert_eq!(workspace(&harness, &repo, &ada).await, before);
    }
}

/// A report that names an object the store lacks ends the run as an executor failure, writing
/// nothing.
pub async fn a_report_naming_a_missing_object_ends_as_executor_failed<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let before = workspace(&harness, &repo, &ada).await;
    let never = Address::of(b"never uploaded");

    harness
        .jobs()
        .report_run(
            &bot,
            run.id,
            exited(vec![("src/a", Change::File(regular(never)))]),
            1,
        )
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(result_of(&ended).outcome, RunOutcome::ExecutorFailed);
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::ExecutorFailed, &[])
    );
    assert_eq!(workspace(&harness, &repo, &ada).await, before);
}

/// A workspace that was landed and reopened is another instance: the old run's result writes
/// nothing into it and moves none of its cache refs.
pub async fn a_run_of_a_closed_instance_writes_nothing_and_moves_no_ref<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a")],
        &[("src/a", "x"), (RUN_FILE, "caches = [\"target\"]")],
    )
    .await;
    harness.teach_run("caches = [\"target\"]", Ok(settings(&["target"], 600)));
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let closed = workspace(&harness, &repo, &ada).await.instance;
    repo.land(&ada, &draft("work"), "main").await.unwrap();
    repo.stage(&ada, &draft("work"), &[("src/other", "o")])
        .await;
    let reopened = workspace(&harness, &repo, &ada).await;
    let new = repo.upload("new").await;
    let built = repo.upload("built").await;
    let mut report = exited(vec![("src/a", Change::File(regular(new)))]);
    report.caches = vec![ProducedFile {
        path: path("target/x"),
        file: regular(built),
    }];

    harness
        .jobs()
        .report_run(&bot, run.id, report, 1)
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::InstanceClosed, &[])
    );
    assert_eq!(workspace(&harness, &repo, &ada).await, reopened);
    assert_eq!(workspace_cache_ref(&harness, &repo, &ada).await, None);
    let old = harness
        .cache_refs()
        .workspace_cache_ref(repo.id, closed, &path("target"))
        .await
        .unwrap();
    assert_eq!(old, None);
}

/// The cache ref a run reports moves by compare-and-swap against what the run started from: the
/// first run to report moves it, and a run that started from the same state does not displace it.
/// Landing the workspace removes its cache refs.
pub async fn cache_refs_move_by_compare_and_swap_and_a_landing_removes_them<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a")],
        &[("src/a", "x"), (RUN_FILE, "caches = [\"target\"]")],
    )
    .await;
    harness.teach_run("caches = [\"target\"]", Ok(settings(&["target"], 600)));
    let first = start(&repo, &ada, request(None, &[])).await.unwrap();
    let second = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let instance = workspace(&harness, &repo, &ada).await.instance;
    let one = repo.upload("one").await;
    let two = repo.upload("two").await;
    let cache = |content| {
        let mut report = exited(vec![]);
        report.caches = vec![ProducedFile {
            path: path("target/x"),
            file: regular(content),
        }];
        report
    };

    harness
        .jobs()
        .report_run(&bot, first.id, cache(one), 1)
        .await
        .unwrap();
    let moved = workspace_cache_ref(&harness, &repo, &ada).await;
    assert!(moved.is_some());
    harness
        .jobs()
        .report_run(&bot, second.id, cache(two), 1)
        .await
        .unwrap();

    assert_eq!(workspace_cache_ref(&harness, &repo, &ada).await, moved);
    repo.land(&ada, &draft("work"), "main").await.unwrap();
    let landed = harness
        .cache_refs()
        .workspace_cache_ref(repo.id, instance, &path("target"))
        .await
        .unwrap();
    assert_eq!(landed, None);
}

/// A run's files are its output tree, listed under a prefix with a download grant each.
pub async fn a_run_lists_its_output_tree_under_a_prefix<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let one = repo.upload("one").await;
    let two = repo.upload("two").await;
    let mut report = exited(vec![]);
    report.outputs = vec![
        ProducedFile {
            path: path("out/a"),
            file: regular(one),
        },
        ProducedFile {
            path: path("out/sub/b"),
            file: regular(two),
        },
        // Not under a declared output: not part of the tree.
        ProducedFile {
            path: path("elsewhere"),
            file: regular(two),
        },
    ];
    let runs = harness.assembly().runs();
    assert_eq!(
        runs.files(&ada, repo.id, run.id, &Prefix::whole()).await,
        Ok(vec![])
    );

    harness
        .jobs()
        .report_run(&bot, run.id, report, 1)
        .await
        .unwrap();

    let all = runs
        .files(&ada, repo.id, run.id, &Prefix::whole())
        .await
        .unwrap();
    let listed: Vec<_> = all
        .iter()
        .map(|output| (output.path.clone(), output.file.content))
        .collect();
    assert_eq!(listed, vec![(path("out/a"), one), (path("out/sub/b"), two)]);
    let under = runs
        .files(&ada, repo.id, run.id, &Prefix::from(path("out/sub")))
        .await
        .unwrap();
    assert_eq!(under.len(), 1);
    assert_eq!(under[0].path, path("out/sub/b"));
    let bob = harness.caller("bob").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "", Access::Read).await;
    assert_eq!(
        runs.files(&bob, repo.id, run.id, &Prefix::whole()).await,
        Err(RunsError::NotFound(run.id))
    );
}

/// A run's reported streams each come with a download grant once it has ended, and a run that
/// has not ended, or reported none, or is another principal's, has none.
pub async fn a_run_gives_download_grants_for_the_streams_it_reported<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    let runs = harness.assembly().runs();
    assert_eq!(
        runs.streams(&ada, repo.id, &run).await,
        Ok(StreamDownloads::default())
    );
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let out = repo.upload("out").await;
    let mut report = exited(vec![]);
    report.stdout = Some(RunStream {
        address: out,
        cut: false,
    });
    harness
        .jobs()
        .report_run(&bot, run.id, report, 1)
        .await
        .unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    let grants = runs.streams(&ada, repo.id, &ended).await.unwrap();

    assert!(grants.stdout.is_some());
    assert_eq!(grants.stderr, None);
    let bob = harness.caller("bob").await;
    assert_eq!(
        runs.streams(&bob, repo.id, &ended).await,
        Err(RunsError::NotFound(run.id))
    );
}

/// A report is for the kind of job the runner holds: a run reported as a check's job, or a check's
/// job reported as a run, is refused with the kind named wrong, and the job is still held.
pub async fn a_report_of_the_other_kind_is_refused<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();

    let wrong = harness
        .jobs()
        .report(
            &bot,
            JobId::new(run.id.value()),
            passed(),
            Produced::default(),
            1,
        )
        .await;
    assert!(matches!(wrong, Err(JobsError::WrongKind(_))), "{wrong:?}");
    assert!(!read(&harness, &repo, &ada, run.id).await.state.ended());
    harness
        .jobs()
        .report_run(&bot, run.id, exited(vec![]), 1)
        .await
        .unwrap();

    land_checks(&repo, &ada, "checks", &[check("unit")]).await;
    let job = harness.jobs().claim(&bot, 2).await.unwrap().unwrap();
    let wrong = harness
        .jobs()
        .report_run(&bot, RunId::new(job.job.id.value()), exited(vec![]), 3)
        .await;
    assert!(matches!(wrong, Err(JobsError::WrongKind(_))), "{wrong:?}");
}

/// A run that is not heartbeated is offered again once its lease lapses, and ends timed out once
/// its deadline passes, with nothing written.
pub async fn a_run_whose_lease_lapses_is_offered_again_and_one_past_its_deadline_times_out<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    let jobs = harness.jobs();
    jobs.claim_run(&bot, 0).await.unwrap().unwrap();
    assert_eq!(jobs.claim_run(&bot, LEASE - 1).await.unwrap(), None);

    let again = jobs.claim_run(&bot, LEASE + 1).await.unwrap().unwrap();
    assert_eq!(again.run.id, run.id);
    let late = i64::try_from(run.request.timeout.seconds()).unwrap() + LEASE + 2;
    assert_eq!(jobs.claim_run(&bot, late).await.unwrap(), None);

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(ended.state, RunState::TimedOut);
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::TimedOut, &[])
    );
}

/// While the server writes a run's changes back the run is reporting: its principal cannot cancel
/// it, so no cancelled run carries a written change. A run that is only running is cancelled.
pub async fn a_run_being_reported_cannot_be_cancelled<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let runs = harness.assembly().runs();
    let jobs = harness.jobs();
    let reported = start(&repo, &ada, request(None, &[])).await.unwrap();
    let running = start(&repo, &ada, request(None, &[])).await.unwrap();
    let claimed = jobs.claim_run(&bot, 0).await.unwrap().unwrap();
    jobs.claim_run(&bot, 0).await.unwrap().unwrap();
    let holder = [claimed.run.runner.unwrap()];

    let reporting = harness
        .queue()
        .begin_report(reported.id, &holder, 1, LEASE)
        .await
        .unwrap();

    assert_eq!(
        reporting.state,
        RunState::Reporting {
            deadline: 1 + LEASE
        }
    );
    assert_eq!(
        runs.cancel(&ada, repo.id, reported.id).await,
        Err(RunsError::Finished(reported.id))
    );
    let held = read(&harness, &repo, &ada, reported.id).await;
    assert_eq!(held.state, reporting.state);
    assert_eq!(
        jobs.heartbeat(&bot, JobId::new(reported.id.value()), 2)
            .await
            .unwrap(),
        Heartbeat::Continue
    );
    // Control: a run that is only running is cancelled.
    let cancelled = runs.cancel(&ada, repo.id, running.id).await.unwrap();
    assert_eq!(cancelled.state, RunState::Cancelled);
}

/// A run left reporting, as when the server stopped midway, is left alone until its deadline, and
/// then ends as failed in the executor with nothing written.
pub async fn a_run_left_reporting_fails_in_its_executor_once_its_deadline_passes<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let before = workspace(&harness, &repo, &ada).await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    let jobs = harness.jobs();
    let claimed = jobs.claim_run(&bot, 0).await.unwrap().unwrap();
    let holder = [claimed.run.runner.unwrap()];
    harness
        .queue()
        .begin_report(run.id, &holder, 1, LEASE)
        .await
        .unwrap();

    jobs.claim_run(&bot, LEASE).await.unwrap();
    assert_eq!(
        read(&harness, &repo, &ada, run.id).await.state,
        RunState::Reporting {
            deadline: 1 + LEASE
        }
    );
    jobs.claim_run(&bot, 1 + LEASE).await.unwrap();

    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(ended.state, RunState::Finished);
    assert_eq!(result_of(&ended).outcome, RunOutcome::ExecutorFailed);
    assert_eq!(
        result_of(&ended).write_back,
        not_written(NotWritten::ExecutorFailed, &[])
    );
    assert_eq!(workspace(&harness, &repo, &ada).await, before);
}

/// A run that timed out or was cancelled while its runner held it takes that runner's upload grants
/// and one report, which records its streams, counts and output tree; what it writes back stays
/// the stop, and nothing is written.
pub async fn a_stopped_run_takes_the_uploads_and_one_report_of_its_runner_and_writes_nothing<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(&harness, &ada, &[("src/a", "a")], &[("src/a", "x")]).await;
    let before = workspace(&harness, &repo, &ada).await;
    let jobs = harness.jobs();
    for cancelled in [false, true] {
        let run = start(&repo, &ada, request(None, &[])).await.unwrap();
        jobs.claim_run(&bot, 0).await.unwrap().unwrap();
        let now = if cancelled {
            harness
                .assembly()
                .runs()
                .cancel(&ada, repo.id, run.id)
                .await
                .unwrap();
            1
        } else {
            let late = i64::try_from(run.request.timeout.seconds()).unwrap() + 1;
            jobs.claim_run(&bot, late).await.unwrap();
            late
        };
        let (state, outcome, reason) = if cancelled {
            (
                RunState::Cancelled,
                RunOutcome::Cancelled,
                NotWritten::Cancelled,
            )
        } else {
            (
                RunState::TimedOut,
                RunOutcome::TimedOut,
                NotWritten::TimedOut,
            )
        };
        assert_eq!(read(&harness, &repo, &ada, run.id).await.state, state);

        let unseen = Upload {
            address: Address::of(b"unseen"),
            size: 6,
        };
        let grants = jobs
            .upload_grants(&bot, JobId::new(run.id.value()), vec![unseen], now)
            .await
            .unwrap();
        assert_eq!(grants.len(), 1);
        let stdout = repo.upload("so").await;
        let new = repo.upload("new").await;
        let mut report = exited(vec![("src/a", Change::File(regular(new)))]);
        report.outcome = outcome;
        report.exit_code = None;
        report.stdout = Some(RunStream {
            address: stdout,
            cut: true,
        });
        report.objects = ObjectCounts {
            downloaded: 3,
            from_cache: 1,
        };

        jobs.report_run(&bot, run.id, report.clone(), now)
            .await
            .unwrap();

        let ended = read(&harness, &repo, &ada, run.id).await;
        assert_eq!(ended.state, state);
        let result = result_of(&ended);
        assert_eq!(result.outcome, outcome);
        assert_eq!(
            result.stdout,
            Some(RunStream {
                address: stdout,
                cut: true
            })
        );
        assert_eq!(result.objects.downloaded, 3);
        assert!(result.output_tree.is_some());
        assert_eq!(result.changes, BTreeMap::new());
        assert_eq!(result.write_back, not_written(reason, &[]));
        assert_eq!(workspace(&harness, &repo, &ada).await, before);
        // The one report is taken.
        let again = jobs.report_run(&bot, run.id, report, now).await;
        assert!(
            matches!(again, Err(JobsError::Queue(JobQueueError::NotFound(_)))),
            "{again:?}"
        );
    }
}

/// A report whose changes name a path the run was neither sent nor allowed to add, or one under a
/// cache path, is refused and ends nothing; the run is still held.
pub async fn a_report_changing_a_path_the_run_could_not_change_is_refused<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a")],
        &[("src/a", "x"), (RUN_FILE, "caches = [\"src/cache\"]")],
    )
    .await;
    harness.teach_run(
        "caches = [\"src/cache\"]",
        Ok(settings(&["src/cache"], 600)),
    );
    let before = workspace(&harness, &repo, &ada).await;
    let run = start(&repo, &ada, request(None, &["src"])).await.unwrap();
    harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let new = repo.upload("new").await;

    for (changed, text) in [
        (Change::File(regular(new)), "src/cache/x"),
        (Change::Deletion, "elsewhere/gone"),
    ] {
        let refused = harness
            .jobs()
            .report_run(&bot, run.id, exited(vec![(text, changed)]), 1)
            .await;
        assert!(
            matches!(&refused, Err(JobsError::UnboundChanges(found)) if *found == vec![path(text)]),
            "{refused:?}"
        );
        let held = read(&harness, &repo, &ada, run.id).await;
        assert!(matches!(held.state, RunState::Running { .. }), "{held:?}");
    }
    assert_eq!(workspace(&harness, &repo, &ada).await, before);

    // Control: a path under `add` and one that was sent are taken.
    harness
        .jobs()
        .report_run(
            &bot,
            run.id,
            exited(vec![
                ("src/a", Change::File(regular(new))),
                ("src/fresh", Change::File(regular(new))),
            ]),
            1,
        )
        .await
        .unwrap();
    let ended = read(&harness, &repo, &ada, run.id).await;
    assert_eq!(
        result_of(&ended).write_back,
        written(&["src/a", "src/fresh"])
    );
}

/// The cache refs move after a command that exited, whatever its code, and stay where they were
/// after a timeout or a failure in the executor.
pub async fn cache_refs_move_after_a_non_zero_exit_and_stay_after_a_timeout_or_an_executor_failure<
    H: RunnerDeployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let (repo, bot) = runnable(
        &harness,
        &ada,
        &[("src/a", "a")],
        &[("src/a", "x"), (RUN_FILE, "caches = [\"target\"]")],
    )
    .await;
    harness.teach_run("caches = [\"target\"]", Ok(settings(&["target"], 600)));
    let cached = |outcome: RunOutcome, code: Option<i32>, content| {
        let mut report = exited(vec![]);
        report.outcome = outcome;
        report.exit_code = code;
        report.caches = vec![ProducedFile {
            path: path("target/x"),
            file: regular(content),
        }];
        report
    };
    for (outcome, code, text, moves) in [
        (RunOutcome::TimedOut, None, "slow", false),
        (RunOutcome::ExecutorFailed, None, "broken", false),
        (RunOutcome::Exited, Some(3), "failed", true),
    ] {
        let run = start(&repo, &ada, request(None, &[])).await.unwrap();
        harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
        let content = repo.upload(text).await;
        let before = workspace_cache_ref(&harness, &repo, &ada).await;
        assert!(before.is_some());

        harness
            .jobs()
            .report_run(&bot, run.id, cached(outcome, code, content), 1)
            .await
            .unwrap();

        let after = workspace_cache_ref(&harness, &repo, &ada).await;
        if moves {
            assert_ne!(after, before, "{outcome:?}");
        } else {
            assert_eq!(after, before, "{outcome:?}");
        }
    }
}

/// Of the registrations of one runner that may take a job, the one with the lowest identifier
/// takes it, for a run and for a check's job.
pub async fn the_lowest_registration_takes_a_job<H: RunnerDeployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    repo.stage(&ada, &draft("work"), &[("src/a", "a")]).await;
    let bot = H::principal("bot");
    let lowest = harness
        .runners()
        .register(&ada, &organisation("acme"), None, &bot, BTreeSet::new())
        .await
        .unwrap();
    let higher = harness
        .runners()
        .register(&ada, &organisation("acme"), None, &bot, BTreeSet::new())
        .await
        .unwrap();
    assert!(lowest < higher);
    let bot = harness.caller("bot").await;
    let run = start(&repo, &ada, request(None, &[])).await.unwrap();
    land_checks(&repo, &ada, "checks", &[check("unit")]).await;

    let claimed = harness.jobs().claim_run(&bot, 0).await.unwrap().unwrap();
    let job = harness.jobs().claim(&bot, 0).await.unwrap().unwrap();

    assert_eq!(claimed.run.id, run.id);
    assert_eq!(claimed.run.runner, Some(lowest));
    assert_eq!(job.job.runner, Some(lowest));
}
