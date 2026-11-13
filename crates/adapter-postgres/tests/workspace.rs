//! The workspace of ADR 0048 through the PostgreSQL adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::workspace;

#[tokio::test]
async fn opening_a_workspace_again_answers_the_same_base() {
    workspace::opening_a_workspace_again_answers_the_same_base::<Harness>().await;
}

#[tokio::test]
async fn a_snapshot_of_an_add_a_modify_and_a_delete_answers_those_three_statuses() {
    workspace::a_snapshot_of_an_add_a_modify_and_a_delete_answers_those_three_statuses::<Harness>()
        .await;
}

#[tokio::test]
async fn a_snapshot_containing_a_path_the_caller_may_not_write_changes_nothing() {
    workspace::a_snapshot_containing_a_path_the_caller_may_not_write_changes_nothing::<Harness>()
        .await;
}

#[tokio::test]
async fn a_snapshot_with_content_that_was_not_uploaded_changes_nothing() {
    workspace::a_snapshot_with_content_that_was_not_uploaded_changes_nothing::<Harness>().await;
}

#[tokio::test]
async fn restoring_a_path_returns_it_to_its_base() {
    workspace::restoring_a_path_returns_it_to_its_base::<Harness>().await;
}

#[tokio::test]
async fn the_diff_shows_a_changed_line_and_marks_a_binary_path() {
    workspace::the_diff_shows_a_changed_line_and_marks_a_binary_path::<Harness>().await;
}

#[tokio::test]
async fn the_file_listing_returns_every_file_and_refuses_a_caller_lacking_read_on_one() {
    workspace::the_file_listing_returns_every_file_and_refuses_a_caller_lacking_read_on_one::<
        Harness,
    >()
    .await;
}

#[tokio::test]
async fn a_landing_without_a_message_lands_with_the_description() {
    workspace::a_landing_without_a_message_lands_with_the_description::<Harness>().await;
}

#[tokio::test]
async fn a_missing_workspace_is_not_found_by_every_operation_that_needs_one() {
    workspace::a_missing_workspace_is_not_found_by_every_operation_that_needs_one::<Harness>()
        .await;
}

#[tokio::test]
async fn a_workspace_is_opened_by_a_writer_and_read_by_a_reader_of_its_paths() {
    workspace::a_workspace_is_opened_by_a_writer_and_read_by_a_reader_of_its_paths::<Harness>()
        .await;
}
