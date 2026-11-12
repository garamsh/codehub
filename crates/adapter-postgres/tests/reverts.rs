//! Reverting a changeset whose blocking check failed, through the PostgreSQL adapters.

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
async fn a_blocking_check_timed_out_by_a_heartbeat_reverts_its_changeset() {
    reverts::a_blocking_check_timed_out_by_a_heartbeat_reverts_its_changeset::<Harness>().await;
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

#[tokio::test]
async fn two_blocking_failures_reported_together_land_one_revert() {
    reverts::two_blocking_failures_reported_together_land_one_revert::<Harness>().await;
}

#[tokio::test]
async fn a_claim_at_a_later_time_leaves_a_timed_out_blocking_job_to_the_revert() {
    reverts::a_claim_at_a_later_time_leaves_a_timed_out_blocking_job_to_the_revert::<Harness>()
        .await;
}

#[tokio::test]
async fn a_heartbeat_at_a_later_time_leaves_a_timed_out_blocking_job_to_the_revert() {
    reverts::a_heartbeat_at_a_later_time_leaves_a_timed_out_blocking_job_to_the_revert::<Harness>()
        .await;
}

#[tokio::test]
async fn a_reverts_checks_are_queued_and_last_green_waits_for_them() {
    reverts::a_reverts_checks_are_queued_and_last_green_waits_for_them::<Harness>().await;
}

#[tokio::test]
async fn a_revert_that_fails_after_its_reservation_is_recorded_as_failed() {
    reverts::a_revert_that_fails_after_its_reservation_is_recorded_as_failed::<Harness>().await;
}

#[tokio::test]
async fn a_second_failing_blocking_check_does_not_try_a_failed_revert_again() {
    reverts::a_second_failing_blocking_check_does_not_try_a_failed_revert_again::<Harness>().await;
}
