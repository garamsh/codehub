//! Runs started in a workspace, through the in-memory adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::runs;

#[tokio::test]
async fn starting_a_run_needs_the_run_action() {
    runs::starting_a_run_needs_the_run_action::<Harness>().await;
}

#[tokio::test]
async fn an_add_path_outside_the_prefix_is_refused() {
    runs::an_add_path_outside_the_prefix_is_refused::<Harness>().await;
}

#[tokio::test]
async fn every_path_sent_and_the_settings_file_must_be_readable() {
    runs::every_path_sent_and_the_settings_file_must_be_readable::<Harness>().await;
}

#[tokio::test]
async fn a_settings_file_that_does_not_parse_is_refused() {
    runs::a_settings_file_that_does_not_parse_is_refused::<Harness>().await;
}

#[tokio::test]
async fn seeding_a_cache_needs_read_on_the_whole_repository() {
    runs::seeding_a_cache_needs_read_on_the_whole_repository::<Harness>().await;
}

#[tokio::test]
async fn a_cache_main_has_no_ref_for_starts_empty_without_a_read_check() {
    runs::a_cache_main_has_no_ref_for_starts_empty_without_a_read_check::<Harness>().await;
}

#[tokio::test]
async fn another_principals_run_is_not_found() {
    runs::another_principals_run_is_not_found::<Harness>().await;
}

#[tokio::test]
async fn cancelling_ends_a_run_and_the_latest_is_the_latest_ended() {
    runs::cancelling_ends_a_run_and_the_latest_is_the_latest_ended::<Harness>().await;
}
