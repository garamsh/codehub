//! Organisations, repositories, staging and object grants through the driving ports.

#[cfg(test)]
mod common;

use app::domain::address::Address;
use app::domain::changeset::Change;
use app::domain::file::{File, FileKind};
use app::domain::name::{OrganisationName, RepositoryName};
use app::port::driven::draft_store::DraftStoreError;
use app::port::driven::object_store::{GrantMethod, ObjectStore};
use app::port::driven::repository_directory::DirectoryError;
use app::port::driving::repositories::{Repositories, RepositoriesError};
use app::port::driving::source_control::{SourceControl, SourceControlError};
use app::use_case::land::LandError;
use app::use_case::read_file::ReadFileError;
use app::use_case::stage::StageError;
use app::use_case::upload::UploadError;
use common::{Harness, MAX_UPLOAD_SIZE, draft_id};

#[tokio::test]
async fn two_repositories_keep_independent_main_refs() {
    let harness = Harness::default();
    let one = harness.repository("one").await;
    let two = harness.repository("two").await;
    assert_ne!(one.id, two.id);

    let draft = draft_id("ada", "first");
    one.stage(&draft, &[("a", "in one\n")]).await;
    let landed = one.land(&draft, "first").await.unwrap();

    assert_eq!(one.main().await, Some(landed));
    assert_eq!(two.main().await, None);
    assert_eq!(one.read("a").await.as_deref(), Some("in one\n"));
    assert_eq!(two.read("a").await, None);
    // Reading through the port agrees: the file is in one repository and not in the other.
    let path = "a".parse().unwrap();
    assert!(
        harness
            .source_control()
            .read_file(one.id, &path)
            .await
            .is_ok()
    );
    assert_eq!(
        harness.source_control().read_file(two.id, &path).await,
        Err(SourceControlError::ReadFile(ReadFileError::NotFound(path)))
    );

    // Landing in the second leaves the first where it was.
    two.stage(&draft, &[("b", "in two\n")]).await;
    let landed_two = two.land(&draft, "second").await.unwrap();
    assert_eq!(two.main().await, Some(landed_two));
    assert_eq!(one.main().await, Some(landed));
    assert_eq!(two.read("a").await, None);
}

#[tokio::test]
async fn staging_a_file_whose_object_was_never_uploaded_is_rejected() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let other = harness.repository("other").await;
    let draft = draft_id("ada", "draft");
    let content = Address::of(b"bytes");
    let file = Change::File(File {
        kind: FileKind::Regular,
        content,
    });
    let source_control = harness.source_control();
    let path = || "a".parse().unwrap();

    assert_eq!(
        source_control.stage(repo.id, &draft, path(), file).await,
        Err(SourceControlError::Stage(StageError::ObjectMissing(
            content
        )))
    );

    // Control: uploaded to another repository it is still missing here; uploaded here it stages.
    assert_eq!(other.upload("bytes").await, content);
    assert_eq!(
        source_control.stage(repo.id, &draft, path(), file).await,
        Err(SourceControlError::Stage(StageError::ObjectMissing(
            content
        )))
    );
    assert_eq!(repo.upload("bytes").await, content);
    assert_eq!(
        source_control.stage(repo.id, &draft, path(), file).await,
        Ok(())
    );
}

#[tokio::test]
async fn staging_then_landing_then_reading_returns_the_download_grant_of_the_staged_content() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let draft = draft_id("ada", "draft");
    let content = repo.upload("#!/bin/sh\n").await;
    let source_control = harness.source_control();
    let path = "bin/run".parse().unwrap();
    source_control
        .stage(
            repo.id,
            &draft,
            "bin/run".parse().unwrap(),
            Change::File(File {
                kind: FileKind::Executable,
                content,
            }),
        )
        .await
        .unwrap();

    repo.land(&draft, "add run").await.unwrap();
    let read = source_control.read_file(repo.id, &path).await.unwrap();

    assert_eq!(read.kind, FileKind::Executable);
    assert_eq!(read.download.method, GrantMethod::Get);
    let staged = harness
        .objects
        .download_grant(repo.id, content)
        .await
        .unwrap();
    let unrelated = harness
        .objects
        .download_grant(repo.id, Address::of(b"other"))
        .await
        .unwrap();
    assert_eq!(read.download.url, staged.url);
    assert_ne!(read.download.url, unrelated.url);
}

#[tokio::test]
async fn a_staged_deletion_removes_the_file_when_it_lands() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let seed = draft_id("ada", "seed");
    repo.stage(&seed, &[("a", "a\n"), ("b", "b\n")]).await;
    repo.land(&seed, "seed").await.unwrap();

    let delete = draft_id("ada", "delete");
    harness
        .source_control()
        .stage(repo.id, &delete, "a".parse().unwrap(), Change::Deletion)
        .await
        .unwrap();
    repo.land(&delete, "delete a").await.unwrap();

    assert_eq!(repo.read("a").await, None);
    assert_eq!(repo.read("b").await.as_deref(), Some("b\n"));
}

#[tokio::test]
async fn creating_a_repository_or_an_organisation_twice_is_rejected() {
    let harness = Harness::default();
    let repositories = harness.repositories();
    let acme: OrganisationName = "acme".parse().unwrap();
    let other: OrganisationName = "other".parse().unwrap();
    let app: RepositoryName = "app".parse().unwrap();

    assert_eq!(
        repositories.create_repository(&acme, &app).await,
        Err(RepositoriesError::Directory(
            DirectoryError::OrganisationNotFound(acme.clone())
        ))
    );
    repositories.create_organisation(&acme).await.unwrap();
    repositories.create_organisation(&other).await.unwrap();
    let id = repositories.create_repository(&acme, &app).await.unwrap();

    assert_eq!(
        repositories.create_repository(&acme, &app).await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryExists(acme.clone(), app.clone())
        ))
    );
    assert_eq!(
        repositories.create_organisation(&acme).await,
        Err(RepositoriesError::Directory(
            DirectoryError::OrganisationExists(acme.clone())
        ))
    );
    // Controls: the same repository name in another organisation is free, and the first
    // repository is found under its name and still holds its identifier.
    let in_other = repositories.create_repository(&other, &app).await.unwrap();
    assert_ne!(in_other, id);
    assert_eq!(repositories.find_repository(&acme, &app).await, Ok(id));
    assert_eq!(
        repositories
            .find_repository(&acme, &"missing".parse().unwrap())
            .await,
        Err(RepositoriesError::Directory(
            DirectoryError::RepositoryNotFound(acme, "missing".parse().unwrap())
        ))
    );
}

#[tokio::test]
async fn landing_removes_the_draft() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let ada = draft_id("ada", "work");
    let bob = draft_id("bob", "work");
    repo.stage(&ada, &[("a", "a\n")]).await;
    repo.stage(&bob, &[("b", "b\n")]).await;

    repo.land(&ada, "ada's").await.unwrap();

    assert_eq!(
        repo.land(&ada, "ada's again").await,
        Err(SourceControlError::Land(LandError::Drafts(
            DraftStoreError::NotFound(ada)
        )))
    );
    // Control: another principal's draft of the same name was not removed with it.
    assert!(repo.land(&bob, "bob's").await.is_ok());
    assert_eq!(repo.read("b").await.as_deref(), Some("b\n"));
}

#[tokio::test]
async fn an_upload_grant_above_the_maximum_size_is_refused() {
    let harness = Harness::default();
    let repo = harness.repository("acme").await;
    let source_control = harness.source_control();
    let address = Address::of(b"big");

    assert_eq!(
        source_control
            .request_upload(repo.id, address, MAX_UPLOAD_SIZE + 1)
            .await,
        Err(SourceControlError::Upload(UploadError::TooLarge {
            size: MAX_UPLOAD_SIZE + 1,
            max_size: MAX_UPLOAD_SIZE
        }))
    );
    // Control: a grant for the maximum size is issued, to put with that length.
    let grant = source_control
        .request_upload(repo.id, address, MAX_UPLOAD_SIZE)
        .await
        .unwrap();
    assert_eq!(grant.method, GrantMethod::Put);
    assert_eq!(
        grant.headers,
        [("content-length".to_owned(), MAX_UPLOAD_SIZE.to_string())]
    );
}
