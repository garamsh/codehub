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

#[tokio::test]
async fn an_exited_run_writes_its_changes_to_the_workspace() {
    runs::an_exited_run_writes_its_changes_to_the_workspace::<Harness>().await;
}

#[tokio::test]
async fn a_run_that_changed_nothing_is_written_though_the_workspace_moved() {
    runs::a_run_that_changed_nothing_is_written_though_the_workspace_moved::<Harness>().await;
}

#[tokio::test]
async fn a_moved_workspace_takes_no_write_and_the_paths_are_named() {
    runs::a_moved_workspace_takes_no_write_and_the_paths_are_named::<Harness>().await;
}

#[tokio::test]
async fn a_denied_write_writes_nothing_and_names_the_paths() {
    runs::a_denied_write_writes_nothing_and_names_the_paths::<Harness>().await;
}

#[tokio::test]
async fn a_run_that_did_not_exit_writes_nothing() {
    runs::a_run_that_did_not_exit_writes_nothing::<Harness>().await;
}

#[tokio::test]
async fn a_report_naming_a_missing_object_ends_as_executor_failed() {
    runs::a_report_naming_a_missing_object_ends_as_executor_failed::<Harness>().await;
}

#[tokio::test]
async fn a_run_of_a_closed_instance_writes_nothing_and_moves_no_ref() {
    runs::a_run_of_a_closed_instance_writes_nothing_and_moves_no_ref::<Harness>().await;
}

#[tokio::test]
async fn cache_refs_move_by_compare_and_swap_and_a_landing_removes_them() {
    runs::cache_refs_move_by_compare_and_swap_and_a_landing_removes_them::<Harness>().await;
}

#[tokio::test]
async fn a_run_lists_its_output_tree_under_a_prefix() {
    runs::a_run_lists_its_output_tree_under_a_prefix::<Harness>().await;
}

#[tokio::test]
async fn a_report_of_the_other_kind_is_refused() {
    runs::a_report_of_the_other_kind_is_refused::<Harness>().await;
}

#[tokio::test]
async fn a_run_whose_lease_lapses_is_offered_again_and_one_past_its_deadline_times_out() {
    runs::a_run_whose_lease_lapses_is_offered_again_and_one_past_its_deadline_times_out::<Harness>(
    )
    .await;
}
