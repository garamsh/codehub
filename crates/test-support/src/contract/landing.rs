//! Landing drafts on `main` through the in-memory adapters.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use futures_util::future::join;
use tokio::sync::Notify;
use tokio::time::timeout;

use crate::contract::{Deployment, MAX_UPLOAD_SIZE, Repo, draft};
use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::merge::FileConflict;
use app::domain::name::DraftName;
use app::domain::principal::{Caller, Principal};
use app::domain::repository::RepositoryId;
use app::port::driven::authorisation_decision::{
    Action, AuthorisationDecision, AuthorisationError, Decision, Resource,
};
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driving::source_control::{LandError, Landing, PathConflict};
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::landing_attempts::LandingAttempts;
use app::use_case::source_control::{
    AfterLanding, AfterLandingError, SourceControlAdapters, SourceControlService,
};

/// Lands a first changeset writing `files` and checks that `main` points at it.
async fn seed<H: Deployment>(repo: &Repo<'_, H>, ada: &Caller, files: &[(&str, &str)]) -> Address {
    let draft = draft("seed");
    repo.stage(ada, &draft, files).await;
    let seed = repo.land(ada, &draft, "seed").await.unwrap();
    assert_eq!(repo.main().await, Some(seed));
    for (path, content) in files {
        assert_eq!(repo.read(path).await.as_deref(), Some(*content));
    }
    seed
}

/// Drafts changing disjoint paths from one base both land.
pub async fn drafts_changing_disjoint_paths_from_one_base_both_land<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let (left, right) = (draft("left"), draft("right"));
    repo.stage(&ada, &left, &[("a", "a1\n")]).await;
    repo.stage(&ada, &right, &[("b", "b1\n")]).await;

    let first = repo.land(&ada, &left, "left").await.unwrap();
    let second = repo.land(&ada, &right, "right").await.unwrap();

    assert_eq!(repo.main().await, Some(second));
    assert_eq!(
        harness.log().get(repo.id, second).await.unwrap().parents,
        [first]
    );
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
    assert_eq!(repo.read("b").await.as_deref(), Some("b1\n"));
}

/// Drafts changing different lines of one file all land.
pub async fn drafts_changing_different_lines_of_one_file_all_land<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("f", "1\n2\n3\n4\n5\n")]).await;
    let left = draft("left");
    let middle = draft("middle");
    let right = draft("right");
    repo.stage(&ada, &left, &[("f", "one\n2\n3\n4\n5\n")]).await;
    repo.stage(&ada, &middle, &[("f", "1\n2\nthree\n4\n5\n")])
        .await;
    repo.stage(&ada, &right, &[("f", "1\n2\n3\n4\nfive\n")])
        .await;

    repo.land(&ada, &left, "left").await.unwrap();
    repo.land(&ada, &middle, "middle").await.unwrap();
    // The last merges against both earlier landings at once.
    let last = repo.land(&ada, &right, "right").await.unwrap();

    assert_eq!(repo.main().await, Some(last));
    assert_eq!(
        repo.read("f").await.as_deref(),
        Some("one\n2\nthree\n4\nfive\n")
    );
}

/// A draft changing the same lines differently is rejected naming the path.
pub async fn a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("f", "1\n2\n3\n"), ("g", "g\n")]).await;
    let first = draft("first");
    let other_line = draft("other-line");
    let same_line = draft("same-line");
    repo.stage(&ada, &first, &[("f", "1\ntwo\n3\n")]).await;
    repo.stage(&ada, &other_line, &[("f", "one\n2\n3\n")]).await;
    repo.stage(
        &ada,
        &same_line,
        &[("f", "1\ndeux\n3\n"), ("g", "changed\n")],
    )
    .await;
    repo.land(&ada, &first, "first").await.unwrap();

    // Control: a draft from the same base changing another line of the file lands.
    let landed = repo.land(&ada, &other_line, "other line").await.unwrap();
    assert_eq!(repo.read("f").await.as_deref(), Some("one\ntwo\n3\n"));

    assert_eq!(
        repo.land(&ada, &same_line, "same line").await,
        Err(SourceControlError::Land(LandError::Conflicts(
            BTreeMap::from([(
                "f".parse().unwrap(),
                PathConflict::File(FileConflict::Lines)
            )])
        )))
    );
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(repo.read("f").await.as_deref(), Some("one\ntwo\n3\n"));
    assert_eq!(repo.read("g").await.as_deref(), Some("g\n"));
}

/// A file where main made a directory is rejected naming the file.
pub async fn a_file_where_main_made_a_directory_is_rejected_naming_the_file<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let base = seed(&repo, &ada, &[("x", "x\n")]).await;
    let (directory, file) = (draft("directory"), draft("file"));
    repo.stage(&ada, &directory, &[("d/e", "e\n")]).await;
    repo.stage(&ada, &file, &[("d", "d\n")]).await;
    repo.land(&ada, &directory, "directory").await.unwrap();

    // Control: the same draft lands where `main` has no directory at its path.
    let control_harness = H::start().await;
    let control = control_harness.repository(&ada, "acme").await;
    assert_eq!(seed(&control, &ada, &[("x", "x\n")]).await, base);
    control.stage(&ada, &file, &[("d", "d\n")]).await;
    assert!(control.land(&ada, &file, "file").await.is_ok());

    let main = repo.main().await;
    assert_eq!(
        repo.land(&ada, &file, "file").await,
        Err(SourceControlError::Land(LandError::Conflicts(
            BTreeMap::from([("d".parse().unwrap(), PathConflict::FileAgainstDirectory)])
        )))
    );
    assert_eq!(repo.main().await, main);
}

/// A changeset log that, at each compare-and-swap it is asked for, first lands the next of
/// `rivals`, so `main` moves between a landing's read of it and its compare-and-swap. Once the
/// rivals are spent it passes the compare-and-swap through. It counts the compare-and-swaps asked.
struct RivalsLandFirst {
    inner: Arc<dyn ChangesetLog>,
    source_control: SourceControlService,
    repository: RepositoryId,
    rivals: Vec<DraftName>,
    caller: Caller,
    asked: AtomicU32,
}

#[async_trait::async_trait]
impl ChangesetLog for RivalsLandFirst {
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

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        self.inner
            .first_parent_chain(repository, head, base, limit)
            .await
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
        let asked = self.asked.fetch_add(1, Ordering::SeqCst);
        if let Some(rival) = usize::try_from(asked)
            .ok()
            .and_then(|index| self.rivals.get(index))
        {
            let landing = Landing {
                message: "rival".to_owned(),
                timestamp: 0,
            };
            self.source_control
                .land(&self.caller, self.repository, rival, landing)
                .await
                .unwrap();
        }
        self.inner
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

/// An authorisation decision that counts the decisions asked of the one it wraps.
struct CountsDecisions {
    inner: Arc<dyn AuthorisationDecision>,
    asked: AtomicU32,
}

#[async_trait::async_trait]
impl AuthorisationDecision for CountsDecisions {
    async fn decide(
        &self,
        subject: &Principal,
        action: Action,
        resource: &Resource,
    ) -> Result<Decision, AuthorisationError> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.inner.decide(subject, action, resource).await
    }
}

/// `caller` landing `draft` through source control over `log` and `authorisation`, trying at most
/// `attempts` times.
async fn land_through<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    draft: &DraftName,
    log: &Arc<RivalsLandFirst>,
    authorisation: &Arc<CountsDecisions>,
    attempts: u32,
) -> Result<Address, SourceControlError> {
    let harness = repo.harness;
    SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects().clone(),
            log: log.clone(),
            derived: harness.derived().clone(),
            drafts: harness.drafts().clone(),
            cache_refs: harness.cache_refs().clone(),
            directory: harness.directory().clone(),
            authorisation: authorisation.clone(),
        },
        MAX_UPLOAD_SIZE,
    )
    .with_landing_attempts(LandingAttempts::new(NonZeroU32::new(attempts).unwrap()))
    .land(
        caller,
        repo.id,
        draft,
        Landing {
            message: draft.to_string(),
            timestamp: 0,
        },
    )
    .await
}

/// A double that lets `rivals` land first, one at each compare-and-swap.
fn racing<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    rivals: &[&DraftName],
) -> Arc<RivalsLandFirst> {
    Arc::new(RivalsLandFirst {
        inner: repo.harness.log().clone(),
        source_control: repo.harness.source_control(),
        repository: repo.id,
        rivals: rivals.iter().map(|rival| (*rival).clone()).collect(),
        caller: caller.clone(),
        asked: AtomicU32::new(0),
    })
}

/// A counter over `harness`'s authorisation decision.
fn counting<H: Deployment>(harness: &H) -> Arc<CountsDecisions> {
    Arc::new(CountsDecisions {
        inner: harness.authorisation().clone(),
        asked: AtomicU32::new(0),
    })
}

/// Stages `count` rival drafts, each writing a path of its own, and returns their names.
async fn stage_rivals<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    count: usize,
) -> Vec<DraftName> {
    let rivals: Vec<DraftName> = (0..count).map(|n| draft(&format!("rival-{n}"))).collect();
    for (n, rival) in rivals.iter().enumerate() {
        repo.stage(caller, rival, &[(&format!("b{n}"), "b\n")])
            .await;
    }
    rivals
}

/// A landing that loses the compare and swap at every attempt is not applied.
pub async fn a_landing_that_loses_the_compare_and_swap_is_not_applied<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let base = seed(&repo, &ada, &[("a", "a0\n")]).await;
    let late = draft("late");
    repo.stage(&ada, &late, &[("a", "a1\n")]).await;
    let rivals = stage_rivals(&repo, &ada, 5).await;
    let spent = racing(&repo, &ada, &rivals.iter().take(3).collect::<Vec<_>>());
    let one_short = racing(&repo, &ada, &rivals.iter().skip(3).collect::<Vec<_>>());
    let decisions = counting(&harness);

    let lost = land_through(&repo, &ada, &late, &spent, &decisions, 3).await;

    let main = repo.main().await;
    assert_ne!(main, Some(base));
    assert_eq!(
        lost,
        Err(SourceControlError::Land(LandError::MainMoved {
            current: main
        }))
    );
    assert_eq!(spent.asked.load(Ordering::SeqCst), 3);
    assert_eq!(repo.read("a").await.as_deref(), Some("a0\n"));
    assert_eq!(repo.read("b2").await.as_deref(), Some("b\n"));

    // Control: with one swap fewer lost than the bound allows, the same draft lands.
    let landed = land_through(&repo, &ada, &late, &one_short, &decisions, 3)
        .await
        .unwrap();
    assert_eq!(one_short.asked.load(Ordering::SeqCst), 3);
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
}

/// A landing that loses the compare and swap is tried again by the server.
pub async fn a_landing_that_loses_the_compare_and_swap_is_tried_again_by_the_server<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("a", "a0\n")]).await;
    let late = draft("late");
    repo.stage(&ada, &late, &[("a", "a1\n")]).await;
    let rivals = stage_rivals(&repo, &ada, 3).await;
    let losing = racing(&repo, &ada, &rivals.iter().collect::<Vec<_>>());
    let decisions = counting(&harness);

    let landed = land_through(&repo, &ada, &late, &losing, &decisions, 5)
        .await
        .unwrap();

    assert_eq!(losing.asked.load(Ordering::SeqCst), 4);
    assert_eq!(repo.main().await, Some(landed));
    for path in ["b0", "b1", "b2"] {
        assert_eq!(repo.read(path).await.as_deref(), Some("b\n"), "{path}");
    }
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
}

/// The authorisation of a landing is asked once, however often the swap is lost.
pub async fn the_authorisation_of_a_landing_is_asked_once_however_often_the_swap_is_lost<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("a", "a0\n"), ("c", "c0\n")]).await;
    let (quiet, late) = (draft("quiet"), draft("late"));
    repo.stage(&ada, &quiet, &[("c", "c1\n")]).await;
    repo.stage(&ada, &late, &[("a", "a1\n")]).await;
    let rivals = stage_rivals(&repo, &ada, 3).await;
    // Control: a landing that loses no swap asks at least one decision.
    let alone = counting(&harness);
    land_through(&repo, &ada, &quiet, &racing(&repo, &ada, &[]), &alone, 5)
        .await
        .unwrap();
    let once = alone.asked.load(Ordering::SeqCst);
    assert!(once > 0);

    let losing = racing(&repo, &ada, &rivals.iter().collect::<Vec<_>>());
    let decisions = counting(&harness);
    land_through(&repo, &ada, &late, &losing, &decisions, 5)
        .await
        .unwrap();

    assert_eq!(losing.asked.load(Ordering::SeqCst), 4);
    assert_eq!(decisions.asked.load(Ordering::SeqCst), once);
}

/// A conflict that appears on a retry is answered as a conflict naming the path.
pub async fn a_conflict_that_appears_on_a_retry_is_answered_naming_the_path<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("f", "1\n2\n3\n4\n5\n")]).await;
    let (late, same, control, other) = (
        draft("late"),
        draft("same-line"),
        draft("control"),
        draft("other-line"),
    );
    repo.stage(&ada, &late, &[("f", "one\n2\n3\n4\n5\n")]).await;
    repo.stage(&ada, &same, &[("f", "uno\n2\n3\n4\n5\n")]).await;
    repo.stage(&ada, &control, &[("f", "1\n2\nthree\n4\n5\n")])
        .await;
    repo.stage(&ada, &other, &[("f", "1\n2\n3\n4\nfive\n")])
        .await;
    let decisions = counting(&harness);

    // The first attempt loses to a landing on the same line; the retry finds the conflict.
    let conflicting = racing(&repo, &ada, &[&same]);
    let lost = land_through(&repo, &ada, &late, &conflicting, &decisions, 5).await;

    assert_eq!(
        lost,
        Err(SourceControlError::Land(LandError::Conflicts(
            BTreeMap::from([(
                "f".parse().unwrap(),
                PathConflict::File(FileConflict::Lines)
            )])
        )))
    );
    assert_eq!(conflicting.asked.load(Ordering::SeqCst), 1);
    assert_eq!(repo.read("f").await.as_deref(), Some("uno\n2\n3\n4\n5\n"));

    // Control: losing the swap to a landing on another line is merged on the retry.
    let merging = racing(&repo, &ada, &[&other]);
    land_through(&repo, &ada, &control, &merging, &decisions, 5)
        .await
        .unwrap();
    assert_eq!(merging.asked.load(Ordering::SeqCst), 2);
    assert_eq!(
        repo.read("f").await.as_deref(),
        Some("uno\n2\nthree\n4\nfive\n")
    );
}

/// How long the first landing waits for a second to read `main`.
const OVERLAP_WAIT: Duration = Duration::from_secs(1);

/// A changeset log that holds the first landing between its read of `main` and its compare-and-swap
/// until a second landing has read `main` too, or `OVERLAP_WAIT` has passed. It counts the landings
/// in that window, and the most there were at once.
struct CountsLandingsInFlight {
    inner: Arc<dyn ChangesetLog>,
    in_flight: AtomicU32,
    most: AtomicU32,
    held: AtomicBool,
}

#[async_trait::async_trait]
impl ChangesetLog for CountsLandingsInFlight {
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

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        self.inner
            .first_parent_chain(repository, head, base, limit)
            .await
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        let main = self.inner.read_ref(repository, name).await?;
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        Ok(main)
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        if !self.held.swap(true, Ordering::SeqCst) {
            let start = Instant::now();
            while self.most.load(Ordering::SeqCst) < 2 && start.elapsed() < OVERLAP_WAIT {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }
        let swapped = self
            .inner
            .compare_and_swap(repository, name, expected, new)
            .await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        swapped
    }
}

/// Lands the draft of each of `landings` at once through one source control, and returns the most
/// landings that were between their read of `main` and their compare-and-swap at once.
async fn most_landing_at_once<H: Deployment>(
    harness: &H,
    caller: &Caller,
    landings: [(RepositoryId, &DraftName); 2],
) -> u32 {
    let log = Arc::new(CountsLandingsInFlight {
        inner: harness.log().clone(),
        in_flight: AtomicU32::new(0),
        most: AtomicU32::new(0),
        held: AtomicBool::new(false),
    });
    let service = SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects().clone(),
            log: log.clone(),
            derived: harness.derived().clone(),
            drafts: harness.drafts().clone(),
            cache_refs: harness.cache_refs().clone(),
            directory: harness.directory().clone(),
            authorisation: harness.authorisation().clone(),
        },
        MAX_UPLOAD_SIZE,
    );
    let [(first, first_draft), (second, second_draft)] = landings;
    let landing = |draft: &DraftName| Landing {
        message: draft.to_string(),
        timestamp: 0,
    };
    let (one, other) = join(
        service.land(caller, first, first_draft, landing(first_draft)),
        service.land(caller, second, second_draft, landing(second_draft)),
    )
    .await;
    one.unwrap();
    other.unwrap();
    log.most.load(Ordering::SeqCst)
}

/// Two landings on one repository at once do not rebase and swap together.
pub async fn two_landings_on_one_repository_at_once_do_not_rebase_and_swap_together<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let other = harness.repository(&ada, "beta").await;
    let (left, right, third) = (draft("left"), draft("right"), draft("third"));
    repo.stage(&ada, &left, &[("a", "a1\n")]).await;
    repo.stage(&ada, &right, &[("b", "b1\n")]).await;
    repo.stage(&ada, &third, &[("c", "c1\n")]).await;
    other.stage(&ada, &left, &[("a", "a1\n")]).await;

    // Control: landings on two repositories do overlap, so the count can see an overlap.
    let apart = most_landing_at_once(&harness, &ada, [(repo.id, &left), (other.id, &left)]).await;
    let same = most_landing_at_once(&harness, &ada, [(repo.id, &right), (repo.id, &third)]).await;

    assert_eq!(apart, 2);
    assert_eq!(same, 1);
    assert_eq!(repo.read("b").await.as_deref(), Some("b1\n"));
    assert_eq!(repo.read("c").await.as_deref(), Some("c1\n"));
}

/// Source control over `harness`'s adapters, its changeset log replaced by `log`.
fn service_over<H: Deployment>(harness: &H, log: Arc<dyn ChangesetLog>) -> SourceControlService {
    SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects().clone(),
            log,
            derived: harness.derived().clone(),
            drafts: harness.drafts().clone(),
            cache_refs: harness.cache_refs().clone(),
            directory: harness.directory().clone(),
            authorisation: harness.authorisation().clone(),
        },
        MAX_UPLOAD_SIZE,
    )
}

/// How long a step that must not wait on another landing may take.
const STEP_BOUND: Duration = Duration::from_secs(30);

/// Runs after each landing; the first does not finish until it is released.
struct BlocksFirstAfterLanding {
    blocked: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl AfterLanding for BlocksFirstAfterLanding {
    async fn after_landing(
        &self,
        _repository: RepositoryId,
        _landed: Address,
        _timestamp: i64,
    ) -> Result<(), AfterLandingError> {
        if !self.blocked.swap(true, Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        Ok(())
    }
}

/// A landing waiting at the gate is not held behind another landing's `after_landing`.
pub async fn a_landing_waiting_at_the_gate_is_not_held_behind_anothers_after_landing<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let (left, right) = (draft("left"), draft("right"));
    repo.stage(&ada, &left, &[("a", "a1\n")]).await;
    repo.stage(&ada, &right, &[("b", "b1\n")]).await;
    let after = Arc::new(BlocksFirstAfterLanding {
        blocked: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let service =
        Arc::new(service_over(&harness, harness.log().clone()).with_after_landing(after.clone()));
    let landing = |draft: &DraftName| Landing {
        message: draft.to_string(),
        timestamp: 0,
    };

    let first = tokio::spawn({
        let (service, ada, left, landing, id) = (
            service.clone(),
            ada.clone(),
            left.clone(),
            landing(&left),
            repo.id,
        );
        async move { service.land(&ada, id, &left, landing).await }
    });
    timeout(STEP_BOUND, after.entered.notified()).await.unwrap();
    // The first landing has swapped `main` and waits in `after_landing`.
    let second = timeout(
        STEP_BOUND,
        service.land(&ada, repo.id, &right, landing(&right)),
    )
    .await
    .expect("the second landing waited for the first's after_landing")
    .unwrap();
    after.release.notify_one();
    let first = first.await.unwrap().unwrap();

    assert_eq!(repo.main().await, Some(second));
    assert_eq!(
        harness.log().get(repo.id, second).await.unwrap().parents,
        [first]
    );
}

/// A changeset log that counts the chain reads and the single-changeset reads it is asked for.
struct CountsReads {
    inner: Arc<dyn ChangesetLog>,
    chains: AtomicU32,
    gets: AtomicU32,
}

#[async_trait::async_trait]
impl ChangesetLog for CountsReads {
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
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.inner.get(repository, id).await
    }

    async fn first_parent_chain(
        &self,
        repository: RepositoryId,
        head: Option<Address>,
        base: Option<Address>,
        limit: usize,
    ) -> Result<Option<Vec<Changeset>>, ChangesetLogError> {
        self.chains.fetch_add(1, Ordering::SeqCst);
        self.inner
            .first_parent_chain(repository, head, base, limit)
            .await
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
        self.inner
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

/// An attempt at a landing reads the chain from its base to `main` in one call, however many
/// changesets `main` is ahead.
pub async fn an_attempt_reads_the_chain_from_its_base_to_main_in_one_call<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    seed(&repo, &ada, &[("a", "a0\n")]).await;
    let late = draft("late");
    repo.stage(&ada, &late, &[("z", "z1\n")]).await;
    for name in ["one", "two", "three"] {
        let ahead = draft(name);
        repo.stage(&ada, &ahead, &[(name, "x\n")]).await;
        repo.land(&ada, &ahead, name).await.unwrap();
    }
    let log = Arc::new(CountsReads {
        inner: harness.log().clone(),
        chains: AtomicU32::new(0),
        gets: AtomicU32::new(0),
    });
    let service = service_over(&harness, log.clone());

    let landing = Landing {
        message: "late".to_owned(),
        timestamp: 0,
    };
    let landed = service.land(&ada, repo.id, &late, landing).await.unwrap();

    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(log.chains.load(Ordering::SeqCst), 1);
    assert_eq!(log.gets.load(Ordering::SeqCst), 0);
    assert_eq!(repo.read("z").await.as_deref(), Some("z1\n"));
    assert_eq!(repo.read("one").await.as_deref(), Some("x\n"));
}

/// The first-parent chain runs from `head` back to `base`, exclusive, newest first, and follows
/// first parents only.
pub async fn the_first_parent_chain_runs_from_head_back_to_base_exclusive<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let first = seed(&repo, &ada, &[("a", "a0\n")]).await;
    let mut landed = vec![first];
    for name in ["one", "two"] {
        let next = draft(name);
        repo.stage(&ada, &next, &[(name, "x\n")]).await;
        landed.push(repo.land(&ada, &next, name).await.unwrap());
    }
    let (one, two) = (landed[1], landed[2]);
    let log = harness.log();
    let mut changesets = BTreeMap::new();
    for id in [first, one, two] {
        changesets.insert(id, log.get(repo.id, id).await.unwrap());
    }
    let of = |ids: &[Address]| -> Option<Vec<Changeset>> {
        Some(ids.iter().map(|id| changesets[id].clone()).collect())
    };

    let chain = |head, base| log.first_parent_chain(repo.id, head, base, usize::MAX);
    assert_eq!(
        chain(Some(two), Some(first)).await.unwrap(),
        of(&[two, one])
    );
    assert_eq!(
        chain(Some(two), None).await.unwrap(),
        of(&[two, one, first])
    );
    assert_eq!(chain(Some(two), Some(two)).await.unwrap(), Some(Vec::new()));
    assert_eq!(chain(None, None).await.unwrap(), Some(Vec::new()));
    // Control: a base that is not behind the head is not found, whichever way round.
    assert_eq!(chain(Some(first), Some(two)).await.unwrap(), None);
    assert_eq!(chain(None, Some(first)).await.unwrap(), None);
    // A base the log does not hold is not found, as a changeset the chain passes through is not.
    let elsewhere = Address::of(b"elsewhere");
    assert_eq!(
        chain(Some(two), Some(elsewhere)).await,
        Err(ChangesetLogError::NotFound(elsewhere))
    );
    assert_eq!(
        chain(None, Some(elsewhere)).await,
        Err(ChangesetLogError::NotFound(elsewhere))
    );
    // No more than `limit` changesets are read: a chain that is longer is an error, and one that
    // is exactly as long is not.
    let bounded = |head, base, limit| log.first_parent_chain(repo.id, head, base, limit);
    assert_eq!(
        bounded(Some(two), Some(first), 2).await.unwrap(),
        of(&[two, one])
    );
    assert!(matches!(
        bounded(Some(two), Some(first), 1).await,
        Err(ChangesetLogError::Backend(_))
    ));
    assert_eq!(
        bounded(Some(two), None, 3).await.unwrap(),
        of(&[two, one, first])
    );
    assert!(matches!(
        bounded(Some(two), None, 2).await,
        Err(ChangesetLogError::Backend(_))
    ));
    assert_eq!(
        bounded(Some(two), Some(two), 0).await.unwrap(),
        Some(Vec::new())
    );
    let unknown = Address::of(b"unknown");
    assert_eq!(
        chain(Some(unknown), None).await,
        Err(ChangesetLogError::NotFound(unknown))
    );

    // A second parent is not followed.
    let merge = Changeset {
        parents: vec![one, two],
        author: "ada".to_owned(),
        timestamp: 0,
        message: "merge".to_owned(),
        changes: BTreeMap::new(),
    };
    let merged = log.append(repo.id, merge.clone()).await.unwrap();
    let mut expected = of(&[one]).unwrap();
    expected.insert(0, merge);
    assert_eq!(
        chain(Some(merged), Some(first)).await.unwrap(),
        Some(expected)
    );
}
