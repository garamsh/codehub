//! Checks queued on landing and run by runners, through the in-memory adapters.

#[cfg(test)]
pub mod common;

use common::Harness;
use test_support::contract::checks;

#[tokio::test]
async fn landing_queues_one_job_per_defined_check() {
    checks::landing_queues_one_job_per_defined_check::<Harness>().await;
}

#[tokio::test]
async fn a_landing_without_a_checks_file_queues_nothing_and_is_last_green() {
    checks::a_landing_without_a_checks_file_queues_nothing_and_is_last_green::<Harness>().await;
}

#[tokio::test]
async fn a_landing_with_only_non_blocking_checks_is_last_green() {
    checks::a_landing_with_only_non_blocking_checks_is_last_green::<Harness>().await;
}

#[tokio::test]
async fn an_unreadable_checks_file_gives_one_failed_blocking_job() {
    checks::an_unreadable_checks_file_gives_one_failed_blocking_job::<Harness>().await;
}

#[tokio::test]
async fn last_green_moves_once_every_blocking_check_passed() {
    checks::last_green_moves_once_every_blocking_check_passed::<Harness>().await;
}

#[tokio::test]
async fn last_green_never_moves_backward() {
    checks::last_green_never_moves_backward::<Harness>().await;
}

#[tokio::test]
async fn a_heartbeat_extends_the_lease() {
    checks::a_heartbeat_extends_the_lease::<Harness>().await;
}

#[tokio::test]
async fn a_job_whose_lease_lapses_is_queued_again() {
    checks::a_job_whose_lease_lapses_is_queued_again::<Harness>().await;
}

#[tokio::test]
async fn a_job_past_its_deadline_times_out() {
    checks::a_job_past_its_deadline_times_out::<Harness>().await;
}

#[tokio::test]
async fn a_cancelled_job_tells_its_runner_to_stop() {
    checks::a_cancelled_job_tells_its_runner_to_stop::<Harness>().await;
}

#[tokio::test]
async fn checks_are_visible_to_readers_and_cancelled_by_administrators() {
    checks::checks_are_visible_to_readers_and_cancelled_by_administrators::<Harness>().await;
}

#[tokio::test]
async fn only_a_failed_blocking_check_keeps_a_changeset_from_last_green() {
    checks::only_a_failed_blocking_check_keeps_a_changeset_from_last_green::<Harness>().await;
}

#[tokio::test]
async fn a_heartbeat_past_the_deadline_answers_timed_out() {
    checks::a_heartbeat_past_the_deadline_answers_timed_out::<Harness>().await;
}

#[tokio::test]
async fn main_is_served_as_a_ref() {
    checks::main_is_served_as_a_ref::<Harness>().await;
}

#[tokio::test]
async fn checks_files_in_every_folder_queue_their_checks() {
    checks::checks_files_in_every_folder_queue_their_checks::<Harness>().await;
}

#[tokio::test]
async fn a_queued_job_carries_the_needs_of_its_check() {
    checks::a_queued_job_carries_the_needs_of_its_check::<Harness>().await;
}

#[tokio::test]
async fn a_need_naming_no_check_gives_one_failed_blocking_job() {
    checks::a_need_naming_no_check_gives_one_failed_blocking_job::<Harness>().await;
}

#[tokio::test]
async fn a_cycle_of_needs_gives_one_failed_blocking_job() {
    checks::a_cycle_of_needs_gives_one_failed_blocking_job::<Harness>().await;
}

#[tokio::test]
async fn a_landing_that_changes_one_folder_queues_only_its_checks() {
    checks::a_landing_that_changes_one_folder_queues_only_its_checks::<Harness>().await;
}

#[tokio::test]
async fn a_check_whose_key_passed_is_not_queued_again() {
    checks::a_check_whose_key_passed_is_not_queued_again::<Harness>().await;
}

#[tokio::test]
async fn a_failed_run_is_not_cached() {
    checks::a_failed_run_is_not_cached::<Harness>().await;
}

#[tokio::test]
async fn a_check_is_keyed_over_the_keys_of_the_checks_it_needs() {
    checks::a_check_is_keyed_over_the_keys_of_the_checks_it_needs::<Harness>().await;
}

#[tokio::test]
async fn a_check_is_queued_once_every_check_it_needs_has_passed() {
    checks::a_check_is_queued_once_every_check_it_needs_has_passed::<Harness>().await;
}

#[tokio::test]
async fn a_check_whose_need_failed_is_skipped_and_never_queued() {
    checks::a_check_whose_need_failed_is_skipped_and_never_queued::<Harness>().await;
}

#[tokio::test]
async fn a_skipped_blocking_check_keeps_last_green_back_and_reverts_nothing() {
    checks::a_skipped_blocking_check_keeps_last_green_back_and_reverts_nothing::<Harness>().await;
}

#[tokio::test]
async fn cancelling_a_check_skips_the_checks_that_need_it() {
    checks::cancelling_a_check_skips_the_checks_that_need_it::<Harness>().await;
}
