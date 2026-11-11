//! Landing drafts on `main` through the in-memory adapters.

#[cfg(test)]
pub mod common;

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::merge::FileConflict;
use app::domain::name::DraftName;
use app::domain::principal::Caller;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::land::{LandError, Landing, PathConflict};
use app::use_case::source_control::SourceControlService;
use common::{Harness, MAX_UPLOAD_SIZE, Repo, draft};

/// Lands a first changeset writing `files` and checks that `main` points at it.
#[cfg(test)]
async fn seed(repo: &Repo<'_>, ada: &Caller, files: &[(&str, &str)]) -> Address {
    let draft = draft("seed");
    repo.stage(ada, &draft, files).await;
    let seed = repo.land(ada, &draft, "seed").await.unwrap();
    assert_eq!(repo.main().await, Some(seed));
    for (path, content) in files {
        assert_eq!(repo.read(path).await.as_deref(), Some(*content));
    }
    seed
}

#[tokio::test]
async fn drafts_changing_disjoint_paths_from_one_base_both_land() {
    let harness = Harness::new().await;
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
        harness.log.get(repo.id, second).await.unwrap().parents,
        [first]
    );
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
    assert_eq!(repo.read("b").await.as_deref(), Some("b1\n"));
}

#[tokio::test]
async fn drafts_changing_different_lines_of_one_file_all_land() {
    let harness = Harness::new().await;
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

#[tokio::test]
async fn a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path() {
    let harness = Harness::new().await;
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

#[tokio::test]
async fn a_file_where_main_made_a_directory_is_rejected_naming_the_file() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let base = seed(&repo, &ada, &[("x", "x\n")]).await;
    let (directory, file) = (draft("directory"), draft("file"));
    repo.stage(&ada, &directory, &[("d/e", "e\n")]).await;
    repo.stage(&ada, &file, &[("d", "d\n")]).await;
    repo.land(&ada, &directory, "directory").await.unwrap();

    // Control: the same draft lands where `main` has no directory at its path.
    let control_harness = Harness::new().await;
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

/// A changeset log that lands a rival draft just before each compare-and-swap it is asked for,
/// so `main` moves between a landing's read of it and its compare-and-swap.
#[cfg(test)]
struct RivalLandsFirst<'a> {
    repo: &'a Repo<'a>,
    rival: DraftName,
    caller: &'a Caller,
}

#[cfg(test)]
impl ChangesetLog for RivalLandsFirst<'_> {
    async fn append(
        &self,
        repository: RepositoryId,
        changeset: Changeset,
    ) -> Result<Address, ChangesetLogError> {
        self.repo.harness.log.append(repository, changeset).await
    }

    async fn get(
        &self,
        repository: RepositoryId,
        id: Address,
    ) -> Result<Changeset, ChangesetLogError> {
        self.repo.harness.log.get(repository, id).await
    }

    async fn read_ref(
        &self,
        repository: RepositoryId,
        name: Ref,
    ) -> Result<Option<Address>, ChangesetLogError> {
        self.repo.harness.log.read_ref(repository, name).await
    }

    async fn compare_and_swap(
        &self,
        repository: RepositoryId,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        self.repo
            .land(self.caller, &self.rival, "rival")
            .await
            .unwrap();
        self.repo
            .harness
            .log
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

#[tokio::test]
async fn a_landing_that_loses_the_compare_and_swap_is_not_applied() {
    let harness = Harness::new().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let base = seed(&repo, &ada, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let (rival, late) = (draft("rival"), draft("late"));
    repo.stage(&ada, &rival, &[("b", "b1\n")]).await;
    repo.stage(&ada, &late, &[("a", "a1\n")]).await;
    let racing = RivalLandsFirst {
        repo: &repo,
        rival,
        caller: &ada,
    };

    let lost = SourceControlService::new(
        &harness.objects,
        &racing,
        &harness.derived,
        &harness.drafts,
        &harness.directory,
        &harness.authorisation,
        MAX_UPLOAD_SIZE,
    )
    .land(
        &ada,
        repo.id,
        &late,
        Landing {
            message: "late".to_owned(),
            timestamp: 0,
        },
    )
    .await;

    let main = repo.main().await;
    assert_ne!(main, Some(base));
    assert_eq!(
        lost,
        Err(SourceControlError::Land(LandError::MainMoved {
            current: main
        }))
    );
    assert_eq!(repo.read("a").await.as_deref(), Some("a0\n"));
    assert_eq!(repo.read("b").await.as_deref(), Some("b1\n"));

    // Control: the same draft lands once nothing moves `main` under it.
    let landed = repo.land(&ada, &late, "late").await.unwrap();
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
}
