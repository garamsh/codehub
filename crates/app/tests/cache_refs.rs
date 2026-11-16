//! The cache refs of a workspace instance, through the in-memory adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::cache_refs;

#[tokio::test]
async fn seeding_copies_mains_ref_and_leaves_it() {
    cache_refs::seeding_copies_mains_ref_and_leaves_it::<Harness>().await;
}

#[tokio::test]
async fn a_seeded_ref_is_not_seeded_again() {
    cache_refs::a_seeded_ref_is_not_seeded_again::<Harness>().await;
}

#[tokio::test]
async fn a_move_from_a_stale_tree_leaves_the_newer_tree() {
    cache_refs::a_move_from_a_stale_tree_leaves_the_newer_tree::<Harness>().await;
}

#[tokio::test]
async fn removal_removes_only_that_instances_refs() {
    cache_refs::removal_removes_only_that_instances_refs::<Harness>().await;
}
