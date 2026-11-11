//! Landing drafts on `main` through the in-memory adapters.

#[cfg(test)]
mod common;

use std::collections::BTreeMap;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::merge::FileConflict;
use app::domain::tree::Entry;
use app::port::driven::changeset_log::{ChangesetLog, ChangesetLogError, Ref};
use app::port::driven::derived_data::DerivedData;
use app::port::driven::object_store::ObjectStore;
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::land::{LandError, PathConflict};
use app::use_case::source_control::SourceControlService;
use common::Workspace;

/// The content of the file at `path` in `main`'s tree, if one is there.
#[cfg(test)]
async fn read(workspace: &Workspace, path: &str) -> Option<String> {
    let path: app::domain::path::Path = path.parse().unwrap();
    let main = workspace.main().await?;
    let mut tree = workspace.derived.root(main).await.unwrap();
    let (name, directories) = path.components().split_last().unwrap();
    for directory in directories {
        match workspace
            .derived
            .tree(tree)
            .await
            .unwrap()
            .entries
            .get(directory)
        {
            Some(Entry::Directory(subtree)) => tree = *subtree,
            Some(Entry::File(_)) | None => return None,
        }
    }
    match workspace
        .derived
        .tree(tree)
        .await
        .unwrap()
        .entries
        .get(name)
    {
        Some(Entry::File(file)) => {
            let bytes = workspace.objects.get(file.content).await.unwrap();
            Some(String::from_utf8(bytes).unwrap())
        }
        Some(Entry::Directory(_)) | None => None,
    }
}

/// Lands a first changeset writing `files` and checks that `main` points at it.
#[cfg(test)]
async fn seed(workspace: &Workspace, files: &[(&str, &str)]) -> Address {
    let seed = workspace
        .land(workspace.draft(None, "seed", files).await)
        .await
        .unwrap();
    assert_eq!(workspace.main().await, Some(seed));
    for (path, content) in files {
        assert_eq!(read(workspace, path).await.as_deref(), Some(*content));
    }
    seed
}

#[tokio::test]
async fn drafts_changing_disjoint_paths_from_one_base_both_land() {
    let workspace = Workspace::default();
    let base = seed(&workspace, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let left = workspace.draft(Some(base), "left", &[("a", "a1\n")]).await;
    let right = workspace.draft(Some(base), "right", &[("b", "b1\n")]).await;

    let first = workspace.land(left).await.unwrap();
    let second = workspace.land(right).await.unwrap();

    assert_eq!(workspace.main().await, Some(second));
    assert_eq!(workspace.log.get(second).await.unwrap().parents, [first]);
    assert_eq!(read(&workspace, "a").await.as_deref(), Some("a1\n"));
    assert_eq!(read(&workspace, "b").await.as_deref(), Some("b1\n"));
}

#[tokio::test]
async fn drafts_changing_different_lines_of_one_file_all_land() {
    let workspace = Workspace::default();
    let base = seed(&workspace, &[("f", "1\n2\n3\n4\n5\n")]).await;
    let left = workspace
        .draft(Some(base), "left", &[("f", "one\n2\n3\n4\n5\n")])
        .await;
    let middle = workspace
        .draft(Some(base), "middle", &[("f", "1\n2\nthree\n4\n5\n")])
        .await;
    let right = workspace
        .draft(Some(base), "right", &[("f", "1\n2\n3\n4\nfive\n")])
        .await;

    workspace.land(left).await.unwrap();
    workspace.land(middle).await.unwrap();
    // The last merges against both earlier landings at once.
    let last = workspace.land(right).await.unwrap();

    assert_eq!(workspace.main().await, Some(last));
    assert_eq!(
        read(&workspace, "f").await.as_deref(),
        Some("one\n2\nthree\n4\nfive\n")
    );
}

#[tokio::test]
async fn a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path() {
    let workspace = Workspace::default();
    let base = seed(&workspace, &[("f", "1\n2\n3\n"), ("g", "g\n")]).await;
    let first = workspace
        .draft(Some(base), "first", &[("f", "1\ntwo\n3\n")])
        .await;
    workspace.land(first).await.unwrap();

    // Control: a draft from the same base changing another line of the file lands.
    let other_line = workspace
        .draft(Some(base), "other line", &[("f", "one\n2\n3\n")])
        .await;
    let landed = workspace.land(other_line).await.unwrap();
    assert_eq!(
        read(&workspace, "f").await.as_deref(),
        Some("one\ntwo\n3\n")
    );

    let same_line = workspace
        .draft(
            Some(base),
            "same line",
            &[("f", "1\ndeux\n3\n"), ("g", "changed\n")],
        )
        .await;
    assert_eq!(
        workspace.land(same_line).await,
        Err(SourceControlError::Land(LandError::Conflicts(
            BTreeMap::from([(
                "f".parse().unwrap(),
                PathConflict::File(FileConflict::Lines)
            )])
        )))
    );
    assert_eq!(workspace.main().await, Some(landed));
    assert_eq!(
        read(&workspace, "f").await.as_deref(),
        Some("one\ntwo\n3\n")
    );
    assert_eq!(read(&workspace, "g").await.as_deref(), Some("g\n"));
}

#[tokio::test]
async fn a_file_where_main_made_a_directory_is_rejected_naming_the_file() {
    let workspace = Workspace::default();
    let base = seed(&workspace, &[("x", "x\n")]).await;
    let directory = workspace
        .draft(Some(base), "directory", &[("d/e", "e\n")])
        .await;
    workspace.land(directory).await.unwrap();
    let file = workspace.draft(Some(base), "file", &[("d", "d\n")]).await;

    // Control: the same draft lands where `main` has no directory at its path.
    let control = Workspace::default();
    let control_base = seed(&control, &[("x", "x\n")]).await;
    assert_eq!(control_base, base);
    let control_file = control.draft(Some(base), "file", &[("d", "d\n")]).await;
    assert_eq!(control_file, file);
    assert!(control.land(control_file).await.is_ok());

    let main = workspace.main().await;
    assert_eq!(
        workspace.land(file).await,
        Err(SourceControlError::Land(LandError::Conflicts(
            BTreeMap::from([("d".parse().unwrap(), PathConflict::FileAgainstDirectory)])
        )))
    );
    assert_eq!(workspace.main().await, main);
}

/// A changeset log that lands a rival draft just before each compare-and-swap it is asked for,
/// so `main` moves between a landing's read of it and its compare-and-swap.
#[cfg(test)]
struct RivalLandsFirst<'a> {
    workspace: &'a Workspace,
    rival: Address,
}

#[cfg(test)]
impl ChangesetLog for RivalLandsFirst<'_> {
    async fn append(&self, changeset: Changeset) -> Result<Address, ChangesetLogError> {
        self.workspace.log.append(changeset).await
    }

    async fn get(&self, id: Address) -> Result<Changeset, ChangesetLogError> {
        self.workspace.log.get(id).await
    }

    async fn read_ref(&self, name: Ref) -> Result<Option<Address>, ChangesetLogError> {
        self.workspace.log.read_ref(name).await
    }

    async fn compare_and_swap(
        &self,
        name: Ref,
        expected: Option<Address>,
        new: Address,
    ) -> Result<(), ChangesetLogError> {
        self.workspace.land(self.rival).await.unwrap();
        self.workspace
            .log
            .compare_and_swap(name, expected, new)
            .await
    }
}

#[tokio::test]
async fn a_landing_that_loses_the_compare_and_swap_is_not_applied() {
    let workspace = Workspace::default();
    let base = seed(&workspace, &[("a", "a0\n"), ("b", "b0\n")]).await;
    let rival = workspace.draft(Some(base), "rival", &[("b", "b1\n")]).await;
    let late = workspace.draft(Some(base), "late", &[("a", "a1\n")]).await;
    let racing = RivalLandsFirst {
        workspace: &workspace,
        rival,
    };

    let lost = SourceControlService::new(
        &workspace.objects,
        &racing,
        &workspace.derived,
        &workspace.drafts,
    )
    .land(late)
    .await;

    let main = workspace.main().await;
    assert_ne!(main, Some(base));
    assert_eq!(
        lost,
        Err(SourceControlError::Land(LandError::MainMoved {
            current: main
        }))
    );
    assert_eq!(read(&workspace, "a").await.as_deref(), Some("a0\n"));
    assert_eq!(read(&workspace, "b").await.as_deref(), Some("b1\n"));

    // Control: the same draft lands once nothing moves `main` under it.
    let landed = workspace.land(late).await.unwrap();
    assert_eq!(workspace.main().await, Some(landed));
    assert_eq!(read(&workspace, "a").await.as_deref(), Some("a1\n"));
}
