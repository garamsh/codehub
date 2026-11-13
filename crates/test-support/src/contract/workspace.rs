//! The workspace of ADR 0048, through the source-control port: opening, describing, snapshotting,
//! restoring and diffing a draft, listing a changeset's files, and landing with the description.

use std::collections::BTreeSet;

use crate::contract::access::Admin;
use crate::contract::{Deployment, Repo, draft};
use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::diff::ContentDiff;
use app::domain::draft::{ChangeStatus, DraftId, Edit};
use app::domain::file::{File, FileKind};
use app::domain::path::{Path, Prefix};
use app::domain::principal::Caller;
use app::port::driven::authorisation_decision::Action;
use app::port::driven::changeset_log::ChangesetLog;
use app::port::driven::derived_data::DerivedDataError;
use app::port::driven::draft_store::DraftStoreError;
use app::port::driven::membership::{Access, Role};
use app::port::driven::object_store::{GrantMethod, ObjectStore};
use app::port::driving::access::AccessError;
use app::port::driving::source_control::{
    ListFilesError, ListedFile, SourceControl, SourceControlError, Workspace, WorkspaceError,
};

async fn list<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    changeset: Address,
    prefix: Prefix,
) -> Result<Vec<ListedFile>, SourceControlError> {
    repo.harness
        .source_control()
        .list_files(caller, repo.id, changeset, &prefix)
        .await
}

fn path(text: &str) -> Path {
    text.parse().unwrap()
}

/// Uploads `bytes` to the repository and names them as a file of `kind`.
async fn upload<H: Deployment>(repo: &Repo<'_, H>, bytes: &[u8], kind: FileKind) -> File {
    let content = repo
        .harness
        .objects()
        .put(repo.id, bytes.to_vec())
        .await
        .unwrap();
    File { kind, content }
}

/// A regular file of the text `content`, uploaded.
async fn regular<H: Deployment>(repo: &Repo<'_, H>, content: &str) -> Edit {
    Edit::Record(Change::File(
        upload(repo, content.as_bytes(), FileKind::Regular).await,
    ))
}

/// Applies `edits` to `caller`'s workspace `name` in one snapshot.
async fn snapshot<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    edits: Vec<(&str, Edit)>,
) -> Result<(), SourceControlError> {
    repo.harness
        .source_control()
        .snapshot(
            caller,
            repo.id,
            &draft(name),
            edits
                .into_iter()
                .map(|(text, edit)| (path(text), edit))
                .collect(),
        )
        .await
}

async fn open<H: Deployment>(repo: &Repo<'_, H>, caller: &Caller, name: &str) -> Workspace {
    repo.harness
        .source_control()
        .open_workspace(caller, repo.id, &draft(name))
        .await
        .unwrap()
}

async fn workspace<H: Deployment>(repo: &Repo<'_, H>, caller: &Caller, name: &str) -> Workspace {
    repo.harness
        .source_control()
        .workspace(caller, repo.id, &draft(name))
        .await
        .unwrap()
}

/// What a workspace changes, as `(path, status)` in path order.
fn statuses(workspace: &Workspace) -> Vec<(String, ChangeStatus)> {
    workspace
        .changes
        .iter()
        .map(|changed| (changed.path.to_string(), changed.status))
        .collect()
}

/// Lands a changeset of the regular files `files` through a workspace and returns it.
async fn land_files<H: Deployment>(
    repo: &Repo<'_, H>,
    caller: &Caller,
    name: &str,
    files: &[(&str, &str)],
) -> Address {
    open(repo, caller, name).await;
    let mut edits = Vec::new();
    for (text, content) in files {
        edits.push((*text, regular(repo, content).await));
    }
    snapshot(repo, caller, name, edits).await.unwrap();
    repo.land(caller, &draft(name), "base").await.unwrap()
}

/// Opening a workspace again answers the base it was opened on.
pub async fn opening_a_workspace_again_answers_the_same_base<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;

    // With `main` unset the workspace has no base, and still has none once `main` is set.
    let early = open(&repo, &ada, "early").await;
    assert_eq!(early.base, None);
    assert_eq!(early.description, "");
    assert_eq!(early.changes, []);
    let first = land_files(&repo, &ada, "first", &[("a", "a1\n")]).await;
    assert_eq!(open(&repo, &ada, "early").await.base, None);

    // A workspace opened later is based on `main` as it then stands, and keeps that base.
    assert_eq!(open(&repo, &ada, "late").await.base, Some(first));
    let second = land_files(&repo, &ada, "second", &[("a", "a2\n")]).await;
    assert_ne!(first, second);
    assert_eq!(open(&repo, &ada, "late").await.base, Some(first));
    assert_eq!(workspace(&repo, &ada, "late").await.base, Some(first));

    // Control: a workspace of a name not yet opened is based on the `main` of now.
    assert_eq!(open(&repo, &ada, "newest").await.base, Some(second));
}

/// A snapshot of an add, a modify and a delete answers those three statuses.
pub async fn a_snapshot_of_an_add_a_modify_and_a_delete_answers_those_three_statuses<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land_files(
        &repo,
        &ada,
        "base",
        &[("a", "a1\n"), ("b", "b1\n"), ("keep", "k\n")],
    )
    .await;
    open(&repo, &ada, "w").await;
    let executable = upload(&repo, b"#!/bin/sh\n", FileKind::Executable).await;
    let same_as_base = regular(&repo, "k\n").await;

    snapshot(
        &repo,
        &ada,
        "w",
        vec![
            ("c", Edit::Record(Change::File(executable))),
            ("a", regular(&repo, "a2\n").await),
            ("b", Edit::Record(Change::Deletion)),
            // Neither changes what the base holds, so neither is listed.
            ("keep", same_as_base),
            ("never", Edit::Record(Change::Deletion)),
        ],
    )
    .await
    .unwrap();

    let held = workspace(&repo, &ada, "w").await;
    assert_eq!(
        statuses(&held),
        [
            ("a".to_owned(), ChangeStatus::Modified),
            ("b".to_owned(), ChangeStatus::Deleted),
            ("c".to_owned(), ChangeStatus::Added),
        ]
    );
    let kinds: Vec<FileKind> = held.changes.iter().map(|changed| changed.kind).collect();
    assert_eq!(
        kinds,
        [FileKind::Regular, FileKind::Regular, FileKind::Executable]
    );
    // A deleted path has nothing to download; the others lead to their content.
    let downloads: Vec<bool> = held
        .changes
        .iter()
        .map(|changed| changed.download.is_some())
        .collect();
    assert_eq!(downloads, [true, false, true]);
    let a_download = held.changes[0].download.as_ref().unwrap();
    assert_eq!(a_download.method, GrantMethod::Get);
    assert_eq!(
        a_download.url,
        harness
            .objects()
            .download_grant(repo.id, Address::of(b"a2\n"))
            .await
            .unwrap()
            .url
    );

    // Opening the workspace again answers the same changes.
    assert_eq!(open(&repo, &ada, "w").await, held);
}

/// A snapshot containing a path the caller may not write changes nothing.
pub async fn a_snapshot_containing_a_path_the_caller_may_not_write_changes_nothing<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "src", Access::Write).await;
    open(&repo, &bob, "w").await;

    let denied = snapshot(
        &repo,
        &bob,
        "w",
        vec![
            ("src/a", regular(&repo, "a\n").await),
            ("docs/b", regular(&repo, "b\n").await),
        ],
    )
    .await;
    assert_eq!(
        denied,
        Err(SourceControlError::Access(AccessError::PathsDenied {
            access: Access::Write,
            paths: BTreeSet::from([path("docs/b")]),
        }))
    );
    assert_eq!(workspace(&repo, &bob, "w").await.changes, []);

    // Control: the permitted path alone is applied.
    snapshot(
        &repo,
        &bob,
        "w",
        vec![("src/a", regular(&repo, "a\n").await)],
    )
    .await
    .unwrap();
    assert_eq!(
        statuses(&workspace(&repo, &bob, "w").await),
        [("src/a".to_owned(), ChangeStatus::Added)]
    );
}

/// A snapshot with content that was not uploaded changes nothing.
pub async fn a_snapshot_with_content_that_was_not_uploaded_changes_nothing<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    open(&repo, &ada, "w").await;
    let missing = Address::of(b"never uploaded");
    let absent = File {
        kind: FileKind::Regular,
        content: missing,
    };

    let refused = snapshot(
        &repo,
        &ada,
        "w",
        vec![
            ("a", regular(&repo, "a\n").await),
            ("b", Edit::Record(Change::File(absent))),
        ],
    )
    .await;
    assert_eq!(
        refused,
        Err(SourceControlError::Workspace(
            WorkspaceError::ObjectMissing(missing)
        ))
    );
    assert_eq!(workspace(&repo, &ada, "w").await.changes, []);

    // Control: once uploaded, the same snapshot applies.
    let uploaded = repo.upload("never uploaded").await;
    assert_eq!(uploaded, missing);
    snapshot(
        &repo,
        &ada,
        "w",
        vec![
            ("a", regular(&repo, "a\n").await),
            ("b", Edit::Record(Change::File(absent))),
        ],
    )
    .await
    .unwrap();
    assert_eq!(workspace(&repo, &ada, "w").await.changes.len(), 2);
}

/// Restoring a path returns it to its base, and restoring an unchanged path is not an error.
pub async fn restoring_a_path_returns_it_to_its_base<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    land_files(&repo, &ada, "base", &[("a", "a1\n"), ("b", "b1\n")]).await;
    open(&repo, &ada, "w").await;
    snapshot(
        &repo,
        &ada,
        "w",
        vec![
            ("a", regular(&repo, "a2\n").await),
            ("b", Edit::Record(Change::Deletion)),
            ("c", regular(&repo, "c\n").await),
            ("d", regular(&repo, "d\n").await),
        ],
    )
    .await
    .unwrap();
    assert_eq!(workspace(&repo, &ada, "w").await.changes.len(), 4);
    let source_control = harness.source_control();

    // A modified, a deleted and an added path each return to the base.
    for text in ["a", "b", "c"] {
        source_control
            .restore(&ada, repo.id, &draft("w"), &path(text))
            .await
            .unwrap();
    }
    assert_eq!(
        statuses(&workspace(&repo, &ada, "w").await),
        [("d".to_owned(), ChangeStatus::Added)]
    );

    // A path the workspace does not change, and one the base never held, are not errors.
    for text in ["a", "never"] {
        assert_eq!(
            source_control
                .restore(&ada, repo.id, &draft("w"), &path(text))
                .await,
            Ok(())
        );
    }

    // A restore in a snapshot does the same.
    snapshot(&repo, &ada, "w", vec![("d", Edit::Restore)])
        .await
        .unwrap();
    assert_eq!(workspace(&repo, &ada, "w").await.changes, []);
}

/// The diff shows a changed line and marks a binary path.
pub async fn the_diff_shows_a_changed_line_and_marks_a_binary_path<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let binary = upload(&repo, &[0, 1, 2], FileKind::Regular).await;
    open(&repo, &ada, "base").await;
    snapshot(
        &repo,
        &ada,
        "base",
        vec![
            ("a", regular(&repo, "one\ntwo\nthree\n").await),
            ("bin", Edit::Record(Change::File(binary))),
            ("mode", regular(&repo, "same\n").await),
        ],
    )
    .await
    .unwrap();
    repo.land(&ada, &draft("base"), "base").await.unwrap();

    open(&repo, &ada, "w").await;
    let changed_binary = upload(&repo, &[0, 9], FileKind::Regular).await;
    let executable = upload(&repo, b"same\n", FileKind::Executable).await;
    snapshot(
        &repo,
        &ada,
        "w",
        vec![
            ("a", regular(&repo, "one\n2\nthree\n").await),
            ("bin", Edit::Record(Change::File(changed_binary))),
            ("mode", Edit::Record(Change::File(executable))),
            ("new", regular(&repo, "x\n").await),
        ],
    )
    .await
    .unwrap();
    let source_control = harness.source_control();

    let diffs = source_control
        .diff(&ada, repo.id, &draft("w"), None)
        .await
        .unwrap();
    let by_path = |text: &str| {
        diffs
            .iter()
            .find(|diff| diff.path == path(text))
            .map(|diff| (diff.status, diff.diff.clone()))
    };
    assert_eq!(diffs.len(), 4);
    assert_eq!(
        by_path("a"),
        Some((
            ChangeStatus::Modified,
            ContentDiff::Lines(
                "--- a/a\n+++ b/a\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n".to_owned()
            )
        ))
    );
    assert_eq!(
        by_path("bin"),
        Some((ChangeStatus::Modified, ContentDiff::Binary))
    );
    assert_eq!(
        by_path("mode"),
        Some((ChangeStatus::Modified, ContentDiff::KindOnly))
    );
    assert_eq!(
        by_path("new"),
        Some((
            ChangeStatus::Added,
            ContentDiff::Lines("--- /dev/null\n+++ b/new\n@@ -0,0 +1 @@\n+x\n".to_owned())
        ))
    );

    // Given a path, the diff is of that path alone.
    let one = source_control
        .diff(&ada, repo.id, &draft("w"), Some(&path("a")))
        .await
        .unwrap();
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].path, path("a"));
}

/// The file listing returns every file and refuses a caller lacking read on one.
pub async fn the_file_listing_returns_every_file_and_refuses_a_caller_lacking_read_on_one<
    H: Deployment,
>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let bob = harness.caller("bob").await;
    let repo = harness.repository(&ada, "acme").await;
    let landed = land_files(
        &repo,
        &ada,
        "base",
        &[
            ("src/a", "a\n"),
            ("src/deep/c", "c\n"),
            ("docs/d", "d\n"),
            ("e", "e\n"),
        ],
    )
    .await;
    let source_control = harness.source_control();
    let paths = |files: &[ListedFile]| {
        files
            .iter()
            .map(|file| file.path.to_string())
            .collect::<Vec<_>>()
    };

    let all = list(&repo, &ada, landed, Prefix::whole()).await.unwrap();
    assert_eq!(paths(&all), ["docs/d", "e", "src/a", "src/deep/c"]);
    let a = all.iter().find(|file| file.path == path("src/a")).unwrap();
    assert_eq!(a.kind, FileKind::Regular);
    assert_eq!(a.content, Address::of(b"a\n"));
    assert_eq!(a.download.method, GrantMethod::Get);
    assert_eq!(
        a.download.url,
        harness
            .objects()
            .download_grant(repo.id, a.content)
            .await
            .unwrap()
            .url
    );

    // A prefix keeps whole components, and a prefix naming a file lists that file.
    let src = list(&repo, &ada, landed, Prefix::from(path("src")))
        .await
        .unwrap();
    assert_eq!(paths(&src), ["src/a", "src/deep/c"]);
    let none = list(&repo, &ada, landed, Prefix::from(path("sr")))
        .await
        .unwrap();
    assert_eq!(paths(&none), Vec::<String>::new());
    let file = list(&repo, &ada, landed, Prefix::from(path("src/a")))
        .await
        .unwrap();
    assert_eq!(paths(&file), ["src/a"]);

    // An unknown changeset is not found.
    let unknown = Address::of(b"no such changeset");
    assert_eq!(
        source_control
            .list_files(&ada, repo.id, unknown, &Prefix::whole())
            .await,
        Err(SourceControlError::ListFiles(ListFilesError::Derived(
            DerivedDataError::NotFound(unknown)
        )))
    );

    // Lacking read on one file refuses the whole listing, naming every path it lacks.
    let admin = Admin {
        harness: &harness,
        caller: &ada,
    };
    admin.add(&bob, Role::Member).await;
    admin.grant(&bob, "src", Access::Read).await;
    assert_eq!(
        list(&repo, &bob, landed, Prefix::whole()).await,
        Err(SourceControlError::Access(AccessError::PathsDenied {
            access: Access::Read,
            paths: BTreeSet::from([path("docs/d"), path("e")]),
        }))
    );
    // Control: a prefix holding only readable files lists.
    let readable = list(&repo, &bob, landed, Prefix::from(path("src")))
        .await
        .unwrap();
    assert_eq!(paths(&readable), ["src/a", "src/deep/c"]);
}

/// A landing without a message lands with the workspace's description.
pub async fn a_landing_without_a_message_lands_with_the_description<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let source_control = harness.source_control();
    let message_of = |id: Address| {
        let log = harness.log();
        let repository = repo.id;
        async move { log.get(repository, id).await.unwrap().message }
    };

    // No message: the description is the message.
    open(&repo, &ada, "described").await;
    source_control
        .describe(&ada, repo.id, &draft("described"), "Add a".to_owned())
        .await
        .unwrap();
    snapshot(
        &repo,
        &ada,
        "described",
        vec![("a", regular(&repo, "a\n").await)],
    )
    .await
    .unwrap();
    assert_eq!(
        workspace(&repo, &ada, "described").await.description,
        "Add a"
    );
    let landed = repo.land(&ada, &draft("described"), "").await.unwrap();
    assert_eq!(message_of(landed).await, "Add a");

    // A message wins over the description.
    open(&repo, &ada, "both").await;
    source_control
        .describe(&ada, repo.id, &draft("both"), "ignored".to_owned())
        .await
        .unwrap();
    snapshot(
        &repo,
        &ada,
        "both",
        vec![("b", regular(&repo, "b\n").await)],
    )
    .await
    .unwrap();
    let landed = repo.land(&ada, &draft("both"), "explicit").await.unwrap();
    assert_eq!(message_of(landed).await, "explicit");

    // A draft staged without opening a workspace has an empty description.
    repo.stage(&ada, &draft("staged"), &[("c", "c\n")]).await;
    assert_eq!(workspace(&repo, &ada, "staged").await.description, "");
    let landed = repo.land(&ada, &draft("staged"), "").await.unwrap();
    assert_eq!(message_of(landed).await, "");
}

/// A workspace that does not exist is not found by any operation that needs one.
pub async fn a_missing_workspace_is_not_found_by_every_operation_that_needs_one<H: Deployment>() {
    let harness = H::start().await;
    let ada = harness.caller("ada").await;
    let repo = harness.repository(&ada, "acme").await;
    let source_control = harness.source_control();
    let name = draft("nope");
    let not_found = || {
        SourceControlError::Workspace(WorkspaceError::Drafts(DraftStoreError::NotFound(DraftId {
            principal: ada.principal().clone(),
            name: name.clone(),
        })))
    };

    assert_eq!(
        source_control.workspace(&ada, repo.id, &name).await.err(),
        Some(not_found())
    );
    assert_eq!(
        source_control
            .describe(&ada, repo.id, &name, "d".to_owned())
            .await,
        Err(not_found())
    );
    assert_eq!(
        snapshot(&repo, &ada, "nope", vec![("a", Edit::Restore)]).await,
        Err(not_found())
    );
    assert_eq!(
        source_control
            .restore(&ada, repo.id, &name, &path("a"))
            .await,
        Err(not_found())
    );
    assert_eq!(
        source_control.diff(&ada, repo.id, &name, None).await.err(),
        Some(not_found())
    );

    // Control: opening creates it, and each operation then answers.
    open(&repo, &ada, "nope").await;
    assert!(source_control.workspace(&ada, repo.id, &name).await.is_ok());
    assert_eq!(
        source_control
            .describe(&ada, repo.id, &name, "d".to_owned())
            .await,
        Ok(())
    );
    assert!(
        source_control
            .diff(&ada, repo.id, &name, None)
            .await
            .is_ok()
    );
}

/// A caller who may not write the repository opens and describes no workspace, and one who may
/// not read a path a workspace changes does not read it.
pub async fn a_workspace_is_opened_by_a_writer_and_read_by_a_reader_of_its_paths<H: Deployment>() {
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
    let source_control = harness.source_control();

    assert_eq!(
        source_control
            .open_workspace(&bob, repo.id, &draft("w"))
            .await
            .err(),
        Some(SourceControlError::Access(AccessError::Denied(
            Action::Write
        )))
    );

    // Control: with write on a prefix, bob opens and changes a workspace.
    admin.grant(&bob, "src", Access::Write).await;
    open(&repo, &bob, "w").await;
    snapshot(
        &repo,
        &bob,
        "w",
        vec![("src/a", regular(&repo, "a\n").await)],
    )
    .await
    .unwrap();
    assert_eq!(workspace(&repo, &bob, "w").await.changes.len(), 1);
    assert_eq!(
        source_control
            .describe(&bob, repo.id, &draft("w"), "d".to_owned())
            .await,
        Ok(())
    );

    // Once bob has no access at all to the path, reading the workspace is refused.
    admin.ungrant(&bob, "", Access::Read).await;
    admin.ungrant(&bob, "src", Access::Write).await;
    admin.grant(&bob, "docs", Access::Write).await;
    assert_eq!(
        source_control
            .workspace(&bob, repo.id, &draft("w"))
            .await
            .err(),
        Some(SourceControlError::Access(AccessError::PathsDenied {
            access: Access::Read,
            paths: BTreeSet::from([path("src/a")]),
        }))
    );
}
