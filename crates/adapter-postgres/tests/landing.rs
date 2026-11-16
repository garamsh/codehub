//! Landing drafts on `main` through the PostgreSQL adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::landing;

#[tokio::test]
async fn drafts_changing_disjoint_paths_from_one_base_both_land() {
    landing::drafts_changing_disjoint_paths_from_one_base_both_land::<Harness>().await;
}

#[tokio::test]
async fn drafts_changing_different_lines_of_one_file_all_land() {
    landing::drafts_changing_different_lines_of_one_file_all_land::<Harness>().await;
}

#[tokio::test]
async fn a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path() {
    landing::a_draft_changing_the_same_lines_differently_is_rejected_naming_the_path::<Harness>()
        .await;
}

#[tokio::test]
async fn a_file_where_main_made_a_directory_is_rejected_naming_the_file() {
    landing::a_file_where_main_made_a_directory_is_rejected_naming_the_file::<Harness>().await;
}

#[tokio::test]
async fn a_landing_that_loses_the_compare_and_swap_is_not_applied() {
    landing::a_landing_that_loses_the_compare_and_swap_is_not_applied::<Harness>().await;
}

#[tokio::test]
async fn a_landing_that_loses_the_compare_and_swap_is_tried_again_by_the_server() {
    landing::a_landing_that_loses_the_compare_and_swap_is_tried_again_by_the_server::<Harness>()
        .await;
}

#[tokio::test]
async fn the_authorisation_of_a_landing_is_asked_once_however_often_the_swap_is_lost() {
    landing::the_authorisation_of_a_landing_is_asked_once_however_often_the_swap_is_lost::<Harness>(
    )
    .await;
}

#[tokio::test]
async fn a_conflict_that_appears_on_a_retry_is_answered_naming_the_path() {
    landing::a_conflict_that_appears_on_a_retry_is_answered_naming_the_path::<Harness>().await;
}

#[tokio::test]
async fn two_landings_on_one_repository_at_once_do_not_rebase_and_swap_together() {
    landing::two_landings_on_one_repository_at_once_do_not_rebase_and_swap_together::<Harness>()
        .await;
}

#[tokio::test]
async fn a_landing_waiting_at_the_gate_is_not_held_behind_anothers_after_landing() {
    landing::a_landing_waiting_at_the_gate_is_not_held_behind_anothers_after_landing::<Harness>()
        .await;
}

#[tokio::test]
async fn an_attempt_reads_the_chain_from_its_base_to_main_in_one_call() {
    landing::an_attempt_reads_the_chain_from_its_base_to_main_in_one_call::<Harness>().await;
}

#[tokio::test]
async fn the_first_parent_chain_runs_from_head_back_to_base_exclusive() {
    landing::the_first_parent_chain_runs_from_head_back_to_base_exclusive::<Harness>().await;
}
