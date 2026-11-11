//! Landing drafts on `main` through the in-memory adapters.

#[cfg(test)]
mod common;

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::draft::DraftId;
use app::domain::merge::FileConflict;
use app::domain::repository::RepositoryId;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::land::{LandError, Landing, PathConflict};
use app::use_case::source_control::SourceControlService;
use common::{Harness, MAX_UPLOAD_SIZE, Repo, draft_id};

/// Lands a first changeset writing `files` and checks that `main` points at it.
#[cfg(test)]
async fn seed(repo: &Repo<'_>, files: &[(&str, &str)]) -> Address {
    let draft = draft_id("ada", "seed");
    repo.stage(&draft, files).await;
    let seed = repo.land(&draft, "seed").await.unwrap();
    assert_eq!(repo.main().await, Some(seed));
    for (path, content) in files {
        assert_eq!(repo.read(path).await.as_deref(), Some(*content));
    }
    seed
}

#[tokio::test]
async fn drafts_changing_disjoint_paths_from_one_base_both_land() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    seed(&repo, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let (left, right) = (draft_id("ada", "left"), draft_id("ada", "right"));
    repo.stage(&left, &[("a", "a1\n")]).await;
    repo.stage(&right, &[("b", "b1\n")]).await;

    let first = repo.land(&left, "left").await.unwrap();
    let second = repo.land(&right, "right").await.unwrap();

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
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    seed(&repo, &[("f", "1\n2\n3\n4\n5\n")]).await;
    let left = draft_id("ada", "left");
    let middle = draft_id("ada", "middle");
    let right = draft_id("ada", "right");
    repo.stage(&left, &[("f", "one\n2\n3\n4\n5\n")]).await;
    repo.stage(&middle, &[("f", "1\n2\nthree\n4\n5\n")]).await;
    repo.stage(&right, &[("f", "1\n2\n3\n4\nfive\n")]).await;

    repo.land(&left, "left").await.unwrap();
    repo.land(&middle, "middle").await.unwrap();
    // The last merges against both earlier landings at once.
    let last = repo.land(&right, "right").await.unwrap();

    assert_eq!(repo.main().await, Some(last));
    assert_eq!(
        repo.read("f").await.as_deref(),
        Some("one\n2\nthree\n4\nfive\n")
    );
}

#[tokio::test]
async fn a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    seed(&repo, &[("f", "1\n2\n3\n"), ("g", "g\n")]).await;
    let first = draft_id("ada", "first");
    let other_line = draft_id("ada", "other-line");
    let same_line = draft_id("ada", "same-line");
    repo.stage(&first, &[("f", "1\ntwo\n3\n")]).await;
    repo.stage(&other_line, &[("f", "one\n2\n3\n")]).await;
    repo.stage(&same_line, &[("f", "1\ndeux\n3\n"), ("g", "changed\n")])
        .await;
    repo.land(&first, "first").await.unwrap();

    // Control: a draft from the same base changing another line of the file lands.
    let landed = repo.land(&other_line, "other line").await.unwrap();
    assert_eq!(repo.read("f").await.as_deref(), Some("one\ntwo\n3\n"));

    assert_eq!(
        repo.land(&same_line, "same line").await,
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
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let base = seed(&repo, &[("x", "x\n")]).await;
    let (directory, file) = (draft_id("ada", "directory"), draft_id("ada", "file"));
    repo.stage(&directory, &[("d/e", "e\n")]).await;
    repo.stage(&file, &[("d", "d\n")]).await;
    repo.land(&directory, "directory").await.unwrap();

    // Control: the same draft lands where `main` has no directory at its path.
    let control_harness = Harness::default();
    let control = control_harness.repository("acme").await;
    assert_eq!(seed(&control, &[("x", "x\n")]).await, base);
    control.stage(&file, &[("d", "d\n")]).await;
    assert!(control.land(&file, "file").await.is_ok());

    let main = repo.main().await;
    assert_eq!(
        repo.land(&file, "file").await,
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
    rival: DraftId,
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
        self.repo.land(&self.rival, "rival").await.unwrap();
        self.repo
            .harness
            .log
            .compare_and_swap(repository, name, expected, new)
            .await
    }
}

#[tokio::test]
async fn a_landing_that_loses_the_compare_and_swap_is_not_applied() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let base = seed(&repo, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let (rival, late) = (draft_id("ada", "rival"), draft_id("ada", "late"));
    repo.stage(&rival, &[("b", "b1\n")]).await;
    repo.stage(&late, &[("a", "a1\n")]).await;
    let racing = RivalLandsFirst { repo: &repo, rival };

    let lost = SourceControlService::new(
        &harness.objects,
        &racing,
        &harness.derived,
        &harness.drafts,
        MAX_UPLOAD_SIZE,
    )
    .land(
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
    let landed = repo.land(&late, "late").await.unwrap();
    assert_eq!(repo.main().await, Some(landed));
    assert_eq!(repo.read("a").await.as_deref(), Some("a1\n"));
}
