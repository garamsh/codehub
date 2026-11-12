//! Landing drafts on `main` through the in-memory adapters.

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
