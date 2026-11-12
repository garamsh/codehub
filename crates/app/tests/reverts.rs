//! Reverting a changeset whose blocking check failed, through the in-memory adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::reverts;

#[tokio::test]
async fn a_failing_blocking_check_reverts_its_changeset_as_a_new_changeset() {
    reverts::a_failing_blocking_check_reverts_its_changeset_as_a_new_changeset::<Harness>().await;
}

#[tokio::test]
async fn a_timed_out_blocking_check_reverts_its_changeset() {
    reverts::a_timed_out_blocking_check_reverts_its_changeset::<Harness>().await;
}

#[tokio::test]
async fn a_later_non_overlapping_change_survives_the_revert() {
    reverts::a_later_non_overlapping_change_survives_the_revert::<Harness>().await;
}

#[tokio::test]
async fn a_conflicting_revert_lands_nothing_and_is_recorded() {
    reverts::a_conflicting_revert_lands_nothing_and_is_recorded::<Harness>().await;
}

#[tokio::test]
async fn a_second_failing_blocking_check_does_not_revert_twice() {
    reverts::a_second_failing_blocking_check_does_not_revert_twice::<Harness>().await;
}

#[tokio::test]
async fn a_failing_non_blocking_check_reverts_nothing() {
    reverts::a_failing_non_blocking_check_reverts_nothing::<Harness>().await;
}

#[tokio::test]
async fn a_cancelled_check_reverts_nothing() {
    reverts::a_cancelled_check_reverts_nothing::<Harness>().await;
}

#[tokio::test]
async fn a_failing_check_of_a_revert_changeset_reverts_nothing() {
    reverts::a_failing_check_of_a_revert_changeset_reverts_nothing::<Harness>().await;
}

#[tokio::test]
async fn a_landing_with_unreadable_definitions_is_reverted() {
    reverts::a_landing_with_unreadable_definitions_is_reverted::<Harness>().await;
}

#[tokio::test]
async fn a_runner_reported_timeout_reverts_its_changeset() {
    reverts::a_runner_reported_timeout_reverts_its_changeset::<Harness>().await;
}

#[tokio::test]
async fn a_runner_reported_cancellation_reverts_nothing() {
    reverts::a_runner_reported_cancellation_reverts_nothing::<Harness>().await;
}
