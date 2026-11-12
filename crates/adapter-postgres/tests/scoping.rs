//! Organisations, repositories, staging and object grants through the driving ports.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::scoping;

#[tokio::test]
async fn two_repositories_keep_independent_main_refs() {
    scoping::two_repositories_keep_independent_main_refs::<Harness>().await;
}

#[tokio::test]
async fn staging_a_file_whose_object_was_never_uploaded_is_rejected() {
    scoping::staging_a_file_whose_object_was_never_uploaded_is_rejected::<Harness>().await;
}

#[tokio::test]
async fn staging_then_landing_then_reading_returns_the_download_grant_of_the_staged_content() {
    scoping::staging_then_landing_then_reading_returns_the_download_grant_of_the_staged_content::<
        Harness,
    >()
    .await;
}

#[tokio::test]
async fn a_staged_deletion_removes_the_file_when_it_lands() {
    scoping::a_staged_deletion_removes_the_file_when_it_lands::<Harness>().await;
}

#[tokio::test]
async fn creating_a_repository_or_an_organisation_twice_is_rejected() {
    scoping::creating_a_repository_or_an_organisation_twice_is_rejected::<Harness>().await;
}

#[tokio::test]
async fn landing_removes_the_draft() {
    scoping::landing_removes_the_draft::<Harness>().await;
}

#[tokio::test]
async fn an_upload_grant_above_the_maximum_size_is_refused() {
    scoping::an_upload_grant_above_the_maximum_size_is_refused::<Harness>().await;
}
