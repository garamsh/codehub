//! Landing drafts on `main` through the in-memory adapters.

use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use futures_util::future::join;

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
use app::use_case::source_control::{SourceControlAdapters, SourceControlService};

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
struct RivalsLandFirst<'a, H> {
    repo: &'a Repo<'a, H>,
    rivals: Vec<DraftName>,
    caller: &'a Caller,
    asked: AtomicU32,
}

impl<H: Deployment> ChangesetLog for RivalsLandFirst<'_, H> {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        self.repo.harness.log().append(repository, changeset).await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        self.repo.harness.log().get(repository, id).await
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        self.repo.harness.log().read_ref(repository, name).await
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
            self.repo.land(self.caller, rival, "rival").await.unwrap();
        }
        self.repo
            .harness
            .log()
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

/// An authorisation decision that counts the decisions asked of the one it wraps.
struct CountsDecisions<'a, A> {
    inner: &'a A,
    asked: AtomicU32,
}

impl<A: AuthorisationDecision + Sync> AuthorisationDecision for CountsDecisions<'_, A> {
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
    log: &RivalsLandFirst<'_, H>,
    authorisation: &CountsDecisions<'_, H::Authorisation>,
    attempts: u32,
) -> Result<Address, SourceControlError> {
    let harness = repo.harness;
    SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects(),
            log,
            derived: harness.derived(),
            drafts: harness.drafts(),
            directory: harness.directory(),
            authorisation,
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
fn racing<'a, H: Deployment>(
    repo: &'a Repo<'a, H>,
    caller: &'a Caller,
    rivals: &[&DraftName],
) -> RivalsLandFirst<'a, H> {
    RivalsLandFirst {
        repo,
        rivals: rivals.iter().map(|rival| (*rival).clone()).collect(),
        caller,
        asked: AtomicU32::new(0),
    }
}

/// A counter over `harness`'s authorisation decision.
fn counting<H: Deployment>(harness: &H) -> CountsDecisions<'_, H::Authorisation> {
    CountsDecisions {
        inner: harness.authorisation(),
        asked: AtomicU32::new(0),
    }
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
struct CountsLandingsInFlight<'a, L> {
    inner: &'a L,
    in_flight: AtomicU32,
    most: AtomicU32,
    held: AtomicBool,
}

impl<L: ChangesetLog + Sync> ChangesetLog for CountsLandingsInFlight<'_, L> {
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
    let log = CountsLandingsInFlight {
        inner: harness.log(),
        in_flight: AtomicU32::new(0),
        most: AtomicU32::new(0),
        held: AtomicBool::new(false),
    };
    let service = SourceControlService::new(
        SourceControlAdapters {
            objects: harness.objects(),
            log: &log,
            derived: harness.derived(),
            drafts: harness.drafts(),
            directory: harness.directory(),
            authorisation: harness.authorisation(),
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
